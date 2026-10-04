# Contributing

This file is for anyone sending a change, including agents. Most of it is about length,
because the usual problem with a generated patch is not that it is wrong. It is that the
code is fine and everything written around it is three times longer than it needs to be.

## The crate

`ddpmeta` is one crate: the library (syncframe parser, edit plan, EMDF and its protection) and
the CLI in `src/main.rs`. It has no dependencies, and a change should not add one. The syntax
authority is ETSI TS 102 366 V1.4.1.

MSRV is 1.87, checked with `cargo +1.87 check --all-targets`. Never nightly.

The EMDF keys are configuration and never part of the repository. Tests that need them skip
without them; with a key set, `EMDF_PROTECTION_KEY_FILE=keys.bin cargo test --release` runs
them too. The `embed-key` feature only builds with a key set (`build.rs`), so leave it out of
the checks below.

## Before you open a pull request

```bash
cargo test --release && cargo clippy --release --all-targets -- -D warnings && cargo fmt --check
```

A fix needs a test that **fails without it**. Write the test, undo your change in the working
tree, watch the test fail, put it back. A test that passes either way documents nothing. Say in
the pull request what it prints when it fails, so a reviewer can confirm it without guessing.

A refactor or a move needs the opposite: `show` output and every stripped stream must be
**byte-identical** before and after, across `tests/data`, under both methods. A move that
changes a byte is not a move.

## Commit messages

Semantic subject with a scope, `!` before the parens when it breaks:

```
fix(strip): keep compr when only --rf is given
feat(show): print loudcorrtyp for each loudness payload
refactor!(emdf): take the key set by key_id
```

The body says **why**, in a few short paragraphs at most. It is not a walk through the diff,
a list of the files touched, or a record of what you tried. The reader can see the diff. What
they cannot see is the reason, so write that and stop.

Subject under 72 characters and shorter where it can be, body wrapped at 72.

Each commit builds and passes the tests on its own, and **no commit introduces something a
later commit in the same series removes or fixes**. If review finds a problem in an early
commit, that is where the fix belongs. Fixup commits are fine while review is under way; tidy
the series once, at the end.

Never invent commit metadata. Author dates are when the work happened, not a tidy ladder.

No `Co-Authored-By` lines for tools, and no "generated with" trailers.

## Changelog entries

Two or three sentences in `CHANGELOG.md`, under `[Unreleased]`. Name the command, option or
public item, say what changed, say what a user does differently. Nothing about how it works
inside. Mark a breaking change `**BREAKING**:`. The release notes are built from this file.

## Code comments

Only what the code cannot say itself. A comment earning its place usually explains a constraint,
a reason for an unobvious choice, or a trap the next reader would otherwise fall into. Restating
the line below it is worse than nothing.

The comment that always earns its place here is one line naming where a constant or a syntax
rule comes from, next to it: the clause or table of TS 102 366 that fixes a field width, a
table, a reserved value. Those are what a reviewer checks.

Never commit a comment that names a local path, a private test file, or where you got a fact
from inside a proprietary binary. This repository is public: a comment is published, not just
committed. State the behaviour a stream shows; leave out how it was found. What a decoder does
with a stream, measured from outside, may be cited; say what was measured in the pull request.

## Pull request descriptions

Say what was wrong, what it does now, and how you know. Evidence is welcome and a table of
measurements is welcome: a one-line fix can need real reproduction context, and length spent on
evidence is length well spent.

What is not welcome is the same explanation three times. Decide where each thing belongs, then
do not repeat it: the reason goes in the commit body, the effect on a user goes in the
changelog, the evidence goes in the pull request.

## Review

Every change is verified, not taken on its description. Expect the reviewer to reproduce the
bug, run your test against unfixed code, and compare `show` output and stripped streams. Make
that easy: say which stream shows the problem and what it prints.

That cuts both ways. Check a review comment before acting on it, including one from a tool. A
generated review is usually right about arithmetic and ranges, and often wrong about how much
something matters.
