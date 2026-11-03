# Judging first: design through refutation rounds before implementation

Decision: [ADR 0009](../../adr/0009-refutation-review-before-every-push.md); the review method is on
the [refutation review](refutation-review.md) page. Recorded 2026-11-04.

## The question

Does iterating a design through refutation rounds on paper, before any code exists, reduce the
review and fix-up rounds the implementation needs afterward?

## The method

Three slices, counted from their review reports and landing records:

- **#174 slice 2** (the aux and bus tail as graph nodes, [ADR 0029](../../adr/0029-strip-job-slots.md)
  section 11). The design was settled through seven judging rounds on paper (open criticals per
  round: 7, 5, 4, 2, 1, 1, 0), then implemented.
- **#268** (System Audio input). Implemented first, with no judging rounds.
- **#289** (Linux XEmbed editor). A hunt: the defect was known, the design was not. No judging rounds.

Definitions used for the counts:

- **Review**: one fresh-context pass that ends in a verdict, PASS or FAIL, on a named head. A narrow
  re-review of the fixed points counts. A read-only pass over one concern (for example the callback
  path of #174 slice 2) is noted but not counted.
- **Fix-up**: one round of changes made to answer a review's findings, whether it fixes code, a
  test, or both. A docs-only wording correction folded into a landing is not counted.
- **Slip**: a finding the author could have caught alone. A **design gap** is a finding the judged
  design should have caught: it forces a design change, or it was missed in the rounds. A **design
  conflict** is a finding that contradicts a point the settled design already accepted.

## Result

| | #174 slice 2 | #268 | #289 |
|---|---|---|---|
| Judging rounds before code | 7 | 0 | 0 |
| Reviews | 2 | 7 | 3 |
| Fix-ups | 1 | 6 | 3, plus a test fix |
| Product findings (critical) | 1 | 6 failing reviews, each on the drift and clock rate estimation (control law, callback aliasing, clock domains, stalls, outliers, arrival delay) | 1, a JUCE-style editor treated as embedded, found twice in different forms |
| Test slips | 3 (counters, signed zero, one interleaving witness) | not tallied | 1 (a window-manager timing assumption) |
| Design conflicts | 1, accepted as documented | 0 | 0 |
| Design changes after judging | 0 | not applicable | not applicable |

The #174 slice 2 first review found one product slip (a home strip returning "forgo" in the closed
window kept its previous block, which was then routed and metered), two test slips that were
critical (the counters test did not exercise abandoned work; a float comparison hid a signed zero)
and one minor test slip (one claim interleaving had no regression). The design conflict was a tail
that, with the tail off, is not bit-for-bit the old path: the settled design already authorized
both differences, so it became a known issue and one as-built sentence. One further coverage gap
was deferred to a follow-up issue. The second review passed; a read-only threading pass over the
callback path passed with minors.

## Caveats

- Three data points. Nothing here is a rate.
- Different sizes. #174 slice 2 was about 1,600 lines of product code, 900 of moves and 2,400 of
  tests; the other two were smaller.
- Different implementers and different reviewers.
- #289 is a hunt, not a designed slice. It sits in the table as a second control for "no judging",
  not as a like-for-like comparison.
- The counts come from our own notes. A review that found nothing and a review that was never
  written down look the same in them.

## The decision

ADR and amendment rounds are mandatory before an implementation slice starts: the author revises,
a fresh-context reviewer judges, and the pair repeats until the reviewer reports no criticals and
every remaining point is accepted or refuted with evidence. There is no round cap, because a round
on paper costs less than a round on code. Implementation reviews stay as they are
([ADR 0009](../../adr/0009-refutation-review-before-every-push.md)).

What the rounds bought in #174 slice 2: no design changes after judging, and every critical finding
in implementation review was a slip (code or test), none a gap in the design.

## What would change the conclusion

- A judged design that still needs design changes in implementation review, repeatedly.
- Judged slices that need as many reviews as unjudged slices of the same size.
- Rounds that stop finding criticals early but keep running because the rule has no cap.
- A larger sample where the counts favor implementing first.

Revisit after the next four judged slices and record the counts in this file.
