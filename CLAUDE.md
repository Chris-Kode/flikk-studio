# CLAUDE.md

@AGENTS.md

The rules in [AGENTS.md](AGENTS.md) apply to every task in this repo. The ones
that are easiest to get wrong:

- **Do not write tests.** No `#[test]`, `#[cfg(test)]`, `tests/` directories,
  benches or fixtures — even when an issue's acceptance criteria mention them.
  Skip those criteria and say so in your summary.
- **Async results must still be wanted.** Check what a result was for (request
  counter, turn, project) before applying it, and give every in-progress
  state an exit on panic, cancel and project switch — see "State and async
  work" in AGENTS.md.
- **Let the layout do the explaining.** Avoid unnecessary secondary labels and
  descriptions in the UI — see "UI copy" in AGENTS.md.
