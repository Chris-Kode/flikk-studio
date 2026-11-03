//! The [`FxBatch`] used without a GPU engine: pictures stay in memory,
//! layers are blended by [`composite_cpu`], and programs are unavailable.

use super::cpu::{self, composite_cpu};
use crate::services::{
    CompositeLayer, Frame, FxBatch, FxCall, PendingFrame, PixelOrder, Placement, ServiceError,
    ServiceResult, Surface,
};

#[derive(Default)]
pub(super) struct CpuBatch {
    frames: Vec<Frame>,
}

impl CpuBatch {
    fn push(&mut self, frame: Frame) -> Surface {
        self.frames.push(frame);
        Surface(self.frames.len() - 1)
    }

    fn get(&self, surface: Surface) -> ServiceResult<&Frame> {
        self.frames
            .get(surface.0)
            .ok_or_else(|| ServiceError("no such surface".into()))
    }
}

fn no_engine<T>() -> ServiceResult<T> {
    Err(ServiceError("programs need the GPU engine".into()))
}

impl FxBatch for CpuBatch {
    fn upload(&mut self, frame: &Frame) -> ServiceResult<Surface> {
        Ok(self.push(frame.clone()))
    }

    fn size(&self, surface: Surface) -> (u32, u32) {
        self.frames
            .get(surface.0)
            .map_or((0, 0), |f| (f.width, f.height))
    }

    fn apply(&mut self, _: Surface, _: &[FxCall<'_>]) -> ServiceResult<Surface> {
        no_engine()
    }

    fn transition(&mut self, _: Surface, _: Surface, _: &FxCall<'_>) -> ServiceResult<Surface> {
        no_engine()
    }

    fn generate(&mut self, _: &FxCall<'_>, _: u32, _: u32) -> ServiceResult<Surface> {
        no_engine()
    }

    fn composite(
        &mut self,
        width: u32,
        height: u32,
        background: [u8; 4],
        layers: &[(Surface, Placement)],
    ) -> ServiceResult<Surface> {
        let refs = layers
            .iter()
            .map(|(s, placement)| {
                Ok(CompositeLayer {
                    frame: self.get(*s)?,
                    placement: *placement,
                })
            })
            .collect::<ServiceResult<Vec<_>>>()?;
        let frame = composite_cpu(width, height, background, &refs);
        Ok(self.push(frame))
    }

    fn mask(&mut self, picture: Surface, mask: Surface) -> ServiceResult<Surface> {
        let frame = crate::mask::apply(self.get(picture)?, self.get(mask)?);
        Ok(self.push(frame))
    }

    fn premultiply(&mut self, surface: Surface) -> ServiceResult<Surface> {
        let frame = cpu::premultiplied(self.get(surface)?);
        Ok(self.push(frame))
    }

    fn unpremultiply(&mut self, surface: Surface) -> ServiceResult<Surface> {
        let frame = cpu::unpremultiplied(self.get(surface)?);
        Ok(self.push(frame))
    }

    fn mix(&mut self, from: Surface, to: Surface, progress: f32) -> ServiceResult<Surface> {
        let frame = cpu::mix(self.get(from)?, self.get(to)?, progress);
        Ok(self.push(frame))
    }

    fn submit<'p>(
        mut self: Box<Self>,
        surface: Surface,
        order: PixelOrder,
    ) -> ServiceResult<Box<dyn PendingFrame + 'p>>
    where
        Self: 'p,
    {
        self.get(surface)?;
        let frame = self.frames.swap_remove(surface.0);
        Ok(Box::new(match order {
            PixelOrder::Rgba => frame,
            PixelOrder::Bgra => cpu::swapped(&frame),
        }))
    }
}
