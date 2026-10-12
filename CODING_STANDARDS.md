# Coding standards

Read during review. These are the judgement calls no check can make; everything
mechanical is enforced in `.github/workflows/ci.yml` and `scripts/hooks/`, so
report nothing here that `cargo fmt`, `cargo clippy`, `scripts/check-sim-deps.sh`
or the em-dash step already fails the build over.

`docs/adr/` is **normative**: where code and an ADR disagree, the ADR wins.
`GLOSSARY.md` holds the vocabulary, and each term's `_Avoid_` list binds comments
and identifiers as well as prose.

## Sabotage every new test

A test earns its place by failing when the code it covers is wrong. Write it,
watch it pass, then **deliberately break that code and watch the test fail**,
then restore. State the sabotage you applied and that it was caught.

This is the project's most expensive lesson. Five tickets in a row shipped tests
that passed for the wrong reason, and each was caught by someone sabotaging the
code rather than by reading the test. The failures were not sloppy assertions;
they were tests whose fixture never reached the state they claimed to cover:

- A stability sweep passed because the start position sat on an exact resonance,
  where a runaway configuration reads as amplitude 1.0000.
- An energy budget passed at a tolerance 1,600 times looser than the worst real
  measurement, so a genuine defect of that size would have gone through.
- A determinism run stayed perfectly deterministic with its opening tap
  mis-aimed, so it proved nothing about the mechanic it was named for.

Two habits follow, and both are cheap:

**Derive expected values from the geometry or the thresholds, by hand, never by
running the code and recording what came out.** A value copied from the output
asserts that the code does what it does.

**Assert the fixture reached the state.** A test about a closing impact also
asserts the bodies were closing; a test about a worst case also asserts the
worst case was observed, rather than passing on a comfortable zero.

## Commit as the work goes

Land each coherent piece as its own commit the moment it is green, rather than
batching the branch into one commit at the end.

The reason is environmental rather than aesthetic: five agent sessions on this
project were killed mid-task by API rate limits, and the two that lost work were
the two holding it uncommitted. The three that committed as they went lost
nothing and resumed from their own history. A commit is the only state that
survives the process that made it.

A branch of small commits also makes the next reviewer's job tractable: a
reviewer can read "fix: stop a separating tick eating the one-hit allowance" and
check that one claim, which is not available in a single commit touching ten
files.

## Say which way a changed test went

When a fix makes an existing test fail, decide whether the old assertion
**encoded the bug** or **encoded real intent**, and say which in the report.

Retuning a constant or widening a tolerance to reach green is how a fix becomes
a regression. If a test's intent genuinely conflicts with a fix, that conflict
is the finding: report it rather than resolving it quietly.

## Name the open decisions, and leave them open

This project carries decisions that are deliberately unresolved, recorded in the
ADR that owns them with their candidate resolutions and none chosen. ADR-0011's
struck route into Thrown is the standing example.

Implement none of them, write no test that presumes one, and remove none of
their documentation. When work lands next to such a gap, verify the gap survived
the change rather than assuming it did. If a fix cannot be made without
resolving one, that is the finding.
