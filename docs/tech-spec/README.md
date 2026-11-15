# Tech specs

Engineering decisions that shape more than one crate: what was decided,
why, what was turned down, and what is still open. One file per decision.
The code's own module docs stay the source of truth for details; a spec
explains the choice behind them.

| Spec | Decision |
| --- | --- |
| [GPU turns and the Jobs list](gpu-turns.md) | GPU work takes turns, one unit at a time, and what takes them is listed in the title bar |
| [Blockouts on the GPU](blockout-gpu.md) | The blockout ray tracer runs as WGSL compute, with the CPU renderer as reference and fallback |

## Writing one

- Name the file after the thing decided, in lower case with hyphens.
- Keep the sections: Status, Problem, Decision, How it works, Turned
  down, Verified, Open.
- Say plainly what was not verified. A spec that hides a gap is worse
  than none.
- Update the spec in the same pull request as a change that makes it
  wrong.
