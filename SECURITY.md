# Reporting a security problem

This program opens files that arrive from anywhere: a PDF is a container of
compressed streams, embedded font programs, encryption, and a small
programming language for drawing. A malformed one reaching a parser is the
normal case here, not an exotic one.

## How to report

Use GitHub's **private vulnerability reporting** on this repository
(Security → Report a vulnerability). That reaches the maintainer without the
report being public first.

Please do not open a public issue for a memory-safety, sandbox-escape or
code-execution problem until it has been fixed.

## What is useful in a report

- the file that triggers it, or the smallest file that still does
- the command or the gesture that triggered it
- what you saw, and what you expected instead
- the commit or release you were on (`pdf-app --version`)

A file that makes the program refuse to open it, or that it draws wrongly, is
a bug rather than a vulnerability, and belongs in a normal issue.

## What to expect

An acknowledgement within a few days, and an honest answer about whether and
when it will be fixed. This is one person's project with no security team
behind it; that is stated here rather than implied by silence.

## What this project does to make such problems less likely

- `unsafe_code = "forbid"` across the whole workspace, checked at build time.
  There is no `unsafe` Rust in it, and adding any requires a reviewed change of
  that policy.
- No C library is linked, and there is no `openssl`: the parser, the filters,
  the cryptography, the font programs and the image decoders are all Rust in
  this repository.
- Two fuzz targets run in the completion gate (`./verify.sh`).
- Unsupported or ambiguous input fails closed. A file this cannot understand is
  refused with a reason rather than guessed at.
- Nothing is sent anywhere. The program makes exactly two kinds of network
  request, both of which a person switches on: a check for a new version, and
  whatever the AI panel is asked to send. Documents are never uploaded.
