# Evidence

transit's rule, from the stabilization playbook
(<https://gist.github.com/derekmwright/0d5a880dddba6214883788eab7c2f0c9>): **every escaped defect
produces a gate that would have caught it, and the gate's class joins the protocol.** A fix without
a new gate only fixes one instance.

| File | Topic |
|---|---|
| [`gates.md`](gates.md) | The commands every change runs, plus the crate-specific gates and the target-directory rule. |
| [`refutation-review.md`](refutation-review.md) | A second reviewer tries to break the change; verdict shape; the merge bar. |
| [`judging-first.md`](judging-first.md) | Whether judging a design on paper first reduces review and fix-up rounds: three slices, the counts, the decision. |
| [`escape-record.md`](escape-record.md) | Recording an escaped defect and the gate it produced. |
| [`tests-as-evidence.md`](tests-as-evidence.md) | Writing a test that proves something. |
| [`hardware-and-ci.md`](hardware-and-ci.md) | What counts as hardware evidence; CI gates every pull request and push to `main`. |
| [`2026-11-07-cpal-native-wasapi-name-equality.md`](2026-11-07-cpal-native-wasapi-name-equality.md) | ADR 0046 slice 2: cpal and the native WASAPI enumerator list the same endpoints, defaults, buffer sizes and rates on the maintainer's machine. |

Reading order for a pull request: `gates.md`, then `tests-as-evidence.md`, then
`refutation-review.md`.
