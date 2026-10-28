"""Shared pieces of the model exports: every graph takes 32 kHz mono samples,
so koo-analysis decodes each file once for all three sound models."""

import math
import os

import onnx
import torch
import torch.nn as nn
from torchaudio.functional.functional import _get_sinc_resample_kernel

RATE = 32_000


class Resample(nn.Module):
    """torchaudio's windowed-sinc resampler with its kernel baked in."""

    def __init__(self, orig: int, new: int):
        super().__init__()
        gcd = math.gcd(orig, new)
        self.orig, self.new = orig // gcd, new // gcd
        kernel, self.width = _get_sinc_resample_kernel(
            orig, new, gcd, 6, 0.99, "sinc_interp_hann", None, "cpu", torch.float32
        )
        self.register_buffer("kernel", kernel)

    def forward(self, x: torch.Tensor) -> torch.Tensor:
        """`x` is (batch, samples)."""
        n = x.shape[-1]
        x = nn.functional.pad(x, (self.width, self.width + self.orig)).unsqueeze(1)
        y = nn.functional.conv1d(x, self.kernel, stride=self.orig)
        y = y.transpose(1, 2).reshape(x.shape[0], -1)
        target = (self.new * n + self.orig - 1) // self.orig
        return y[..., :target]


def export(model: nn.Module, args: tuple, path: str, inputs: list, outputs: list, dynamic: dict):
    """Export with the dynamo exporter at opset 18 and check the file loads."""
    model.eval()
    with torch.no_grad():
        program = torch.onnx.export(
            model,
            args,
            input_names=inputs,
            output_names=outputs,
            dynamic_shapes=dynamic,
            opset_version=18,
            dynamo=True,
            external_data=False,
        )
    program.optimize()
    program.save(path, external_data=False)
    onnx.checker.check_model(path, full_check=True)
    print(f"wrote {path}")


def _bare(src: str) -> onnx.ModelProto:
    """The model without the exporter's intermediate shapes, which can
    disagree with ONNX's own inference that the quantizers run."""
    model = onnx.load(src)
    del model.graph.value_info[:]
    return model


def quantize(src: str, dest: str):
    """Dynamic int8 MatMuls: fastest on CPU, for a model run on every
    stretch of audio."""
    from onnxruntime.quantization import QuantType, quantize_dynamic

    bare = dest + ".bare.onnx"
    onnx.save(_bare(src), bare)
    quantize_dynamic(bare, dest, op_types_to_quantize=["MatMul"], weight_type=QuantType.QInt8)
    os.remove(bare)
    print(f"wrote {dest}")


def quantize_weights(src: str, dest: str):
    """8-bit weights in blocks of 32 for the MatMuls (activations stay
    float) and an int8 embedding table: closer to the float model than
    `quantize`, for a model run once per query."""
    from onnxruntime.quantization import QuantType, quantize_dynamic
    from onnxruntime.quantization.matmul_nbits_quantizer import MatMulNBitsQuantizer

    q = MatMulNBitsQuantizer(_bare(src), block_size=32, is_symmetric=True, bits=8,
                             op_types_to_quantize=("MatMul",))
    q.process()
    blocks = dest + ".blocks.onnx"
    q.model.save_model_to_file(blocks, use_external_data_format=False)
    quantize_dynamic(blocks, dest, op_types_to_quantize=["Gather"], weight_type=QuantType.QInt8)
    os.remove(blocks)
    print(f"wrote {dest}")
