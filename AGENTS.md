# transit: instructions for agents

transit is a desktop DAW (Rust, egui) with its own engine. A crash loses someone's recording, so
that outranks feature work.

Before changing code, read:

1. [`docs/contributing/agents.md`](docs/contributing/agents.md): the workflow and the rules agents must know.
2. [`CONTRIBUTING.md`](CONTRIBUTING.md): the gate commands.
3. [ADR 0009](docs/adr/0009-refutation-review-before-every-push.md): refutation review.
4. [ADR 0013](docs/adr/0013-unsafe-only-in-soundcraft-rt.md): where `unsafe` is allowed.

Three rules agents break most often:

- **File budget.** No Rust file over 1500 lines; `cargo xtask file-size` is a ratchet. Never grow a
  file near the budget, split it by concern as a pure move first ([`gates.md`](docs/contributing/evidence/gates.md)).
- **No real-clock assertions.** A test never asserts scheduling or wall-clock outcomes; inject the
  stamps and the readout (`cargo xtask timing`, [escapes](docs/escapes/)).
- **Never rerun a failed test.** Not to pass it, and not to capture its message: read the output,
  report it verbatim and find the root cause before changing the test. A failure on a loaded host is
  not a verdict (rerun on a quiet host); a quiet failure is real.

Then [`docs/contributing/README.md`](docs/contributing/README.md) for the knowledge and evidence
folders, and [`docs/known-issues/README.md`](docs/known-issues/README.md) so you do not rediscover
known defects.
