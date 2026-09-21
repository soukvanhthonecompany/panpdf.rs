# Contributing

Patches, bug reports and documents that reproduce a defect are all welcome.
This file says what the project expects, so that a change does not have to be
sent back for reasons nobody wrote down.

## The rules a change is held to

- `pdf-bytes` imports no other crate in the workspace, and the dependency
  direction between the rest is the one `ARCHITECTURE.md` lists.
- Syntax parsing never repairs input silently. Strict and recovery behaviour
  are separate APIs, and repairs come back as data.
- Parsed values keep their `SourceSpan`; derived values keep their provenance.
- An edit is planned and validated before it mutates a document revision.
- Rendering backends consume the canonical paint graph. Interface code does not
  reproduce PDF paint.
- Unsupported or ambiguous input fails closed, with a reason a person can read.
- No `unsafe` Rust. The workspace forbids it.
- No new dependency without saying, in the change, what it is for, what its
  licence is, what else was considered, and whether it ships or is test-only.
  This project prefers writing the thing to importing it; say why yours is the
  exception.
- Arithmetic belongs in tested modules, not in interface code.
- A regression test must prove its measuring instrument on a known answer
  before its result about a real document means anything. A test that reports
  a number no one has checked against a known case is not a test.

## The gate

```sh
./verify.sh --quick    # formatting, clippy, the tests, the browser target
./verify.sh            # the same, plus a release build of both binaries
```

It stops at the first failure. There is no CI for it, so this script is the
gate, and a change that has not passed it is not ready to look at.

Compilation is not evidence. If your change touches how documents are read,
drawn or written, say what you checked it against -- a document, another
renderer, a measurement -- and paste the output.

## Languages

**The interface is in English, and a patch should add English only.**

Every sentence the window says lives in `crates/pdf-app/src/wording/` as a
variant of an enum rather than as a literal where it is shown, so that a
sentence that is missing does not compile instead of appearing as a key. Adding
one means writing one arm.

The machinery for more languages is deliberately still there -- `Lang`, its tag
and endonym, the `match lang` in every `say`, and the Language entry in the View
menu. It is not hard-coded away, and adding a language means adding a variant to
`Lang` and answering the sentences it names; the compiler lists every one it has
not answered, so a language is finished or it does not exist.

It is not ready for one yet, and it would be unkind not to say so. Two things
have to be fixed first: the interface loads its fonts from paths that exist only
on Linux, so a language outside the Latin alphabet draws as empty boxes on
Windows and macOS; and the interface toolkit does not reorder right-to-left
text, so Arabic and Urdu come out spelled correctly and read backwards. Until
those are paid, a translation would look finished and not be.

## Commit messages

One line that says what a person can now do, or what no longer happens, in
plain words: *"a crop is a window, and a picture slides under it"*, not
*"refactor ClipRegion"*. Then the reasoning, if it needs any.

## Reporting a bug

The most useful report is a file plus one sentence. If the document is one you
cannot share, say what it contains -- a font, an encryption revision, a form --
and what the program did.

Security problems go to [`SECURITY.md`](SECURITY.md) instead.

## Licence of contributions

This project is under the AGPL-3.0 ([`LICENSE`](LICENSE)). By opening a pull
request you offer your change under the same licence.
