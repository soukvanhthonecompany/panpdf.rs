# Architecture

## One canonical state

The canonical state is a versioned PDF byte store plus lossless parsed indexes.
The paint graph, semantic index, selection state, and rendered tiles are all
derived. Only a committed transaction creates a new document revision.

```text
immutable PDF bytes
        |
        v
syntax graph + source spans
        |
        v
content interpreter ---> semantic index
        |
        v
paint graph ---> render device ---> tiles
        ^
        |
edit planner <--- user command
        |
        v
validated source patch ---> new PDF revision
```

The semantic index points to paint atoms. It owns no pixels and no independent
transforms. The browser owns zoom and presentation only.

A revision is held as pieces: the file as it was opened, shared by every
revision after it, and what the session appended. Readers take the bytes a
piece at a time (`ByteStore::run_at`, `resolve`, `get`, `ahead`), so an edit
costs what it writes rather than a copy of the file. `ByteStore::as_bytes`
still answers with one slice, joining the pieces once and keeping the join;
nothing on an edit's path asks for it, and `pdf_bytes::whole_copies` counts
the joins so tests can say so.

## Dependency direction

The workspace has these responsibilities. Every direct dependency one of them is permitted on another is listed at the
end of this file, and no other is allowed.

| Crate | Responsibility |
| --- | --- |
| `pdf-bytes` | Immutable bytes and source spans; no workspace dependency |
| `pdf-syntax` | Lossless syntax, resolution, filters, revisions |
| `pdf-security` | Authentication and encryption over bytes and syntax |
| `pdf-font` | Font programs, metrics, encodings and Unicode evidence |
| `pdf-content` | Page resources and source-identified content operations |
| `pdf-paint` | Interpretation into the canonical sourced paint graph |
| `pdf-render` | Pixels derived from that graph |
| `pdf-semantics` | Clusters, rows and blocks referencing that graph, the audit of whether every atom has exactly one owner, the page-local index of what each object's paint depends on, and what is under a point |
| `pdf-edit` | Commands, plans, source writes and history |
| `pdf-session` | Open document, interpreted pages, invalidation, and revision-bound selection: object references, the frozen candidate stack and frozen gestures |
| `pdf-cli` | Engine-facing queries, verification and the native CLI |
| `pdf-ocr` | Reading the words a scanned page shows, by running an installed recogniser, as a layer for `pdf-edit` to write |
| `pdf-compose` | A written document as Markdown states it: the model a page layout is derived from, with no paint, no file and no dependency |
| `pdf-heap` | Asking the allocator to hand freed memory back to the operating system. The only crate where `unsafe` is allowed, by the design record: four C calls, no dependency, its own lint table so the workspace rule stays `forbid` |
| `pdf-print` | Printing: which pages, on what paper, how large and how many to a sheet; each sheet drawn as it prints; and the job given to the system's print service -- CUPS over IPP, or Windows's spooler |
| `pdf-app` | Editor logic and the window: the product's one front end |
| `pdf-agent` | The engine offered to AI agents over the Model Context Protocol (`panpdf-mcp`): the second front end, with no window |

`/JPXDecode` is a whole second file format rather than a filter, so its decoder
is a library of its own: `jpeg2000`, written here and since 2026-09-21 kept in
its own repository (`github.com/soukvanhthonecompany/jpeg2000`, MIT OR
Apache-2.0), pulled in by `pdf-paint` as a git dependency pinned to a tag. It
knows nothing about PDF and depends on nothing. See the dependency list in `Cargo.toml`.

`pdf-render` and `pdf-semantics` are sibling consumers of `pdf-paint`.
`pdf-font` and `pdf-security` independently depend on bytes and syntax.
There is no `pdf-write` crate today: the incremental writer is in
`pdf-edit::incremental`. The earlier diagram incorrectly implied all three
relationships. `pdf-app` currently uses `pdf-cli`'s library for overlay queries
and verification; this is an explicit existing dependency, not permission for
the lower engine crates to depend on the CLI or a window.

`pdf-ocr` (2026-09-18) sits beside `pdf-cli` above the engine. It draws a page
with `pdf-render`, hands the picture to Tesseract running as a separate
process, and turns what comes back into a `pdf_edit::text_layer::TextLayer`;
`pdf-edit` writes it like any other command, so the recogniser never touches a
document and the engine never starts a process. A recogniser is the one thing
in this product that is not the owner's code (the owner, 2026-09-18: an outside
project may be used, chosen by measurement, and must run with no server), and
keeping it behind a process boundary in a crate of its own is what keeps it
replaceable. `pdf-session` is a test dependency only, for the example that runs
the whole path over a file.

`pdf-print` (2026-09-18) sits beside `pdf-ocr`. Its layout is plain
geometry over page sizes. A sheet is drawn by reading each page for paper
(`pdf_session::interpret_page_for_print`, which reads optional content's print
states and draws only the annotations flagged Print), rendering it with
`pdf-render` at exactly the scale it lands at, and placing whole pixels. So the
preview in the window and what a printer will be sent come from one function.
It writes nothing to a document and depends on no editing crate.

The job goes to the print service over IPP, spoken by `pdf_print::ipp` and
`pdf_print::cups` -- the protocol `lp` itself speaks, so no printing library
and no HTTP crate is linked, and a printer's papers and margins come back as
typed values rather than as sentences to parse. What is sent is a PDF of
pictures of the sheets at the printer's resolution: the print service then has
nothing left to decide, and what comes out is what the preview showed.

Which print service answers is `pdf_print::service`'s to know, and the
dialogue does not. On Linux and macOS it is CUPS, over IPP -- the protocol
`lp` itself speaks, so no printing library and no HTTP crate is linked, and a
printer's papers and margins come back as typed values rather than as
sentences to parse. On Windows (2026-09-19) it is the spooler, reached by
`pdf_print::windows` through a PowerShell script the crate carries and the
printing classes every copy of Windows has. That indirection is what a
C library would cost: calling the spooler's own functions is FFI, which is
`unsafe`, which `CONTRIBUTING.md` forbids without an RFC. It is the same bargain
`pdf_app::system_dialog` strikes when it runs `zenity` for a file window.
Sheets cross to the script as pixels on its standard input, one at a time,
so a long job holds one sheet in memory on either side and writes nothing to
disk.

`pdf-agent` (2026-09-19) sits beside `pdf-app`, as the second front end: the
same engine, driven by an AI agent instead of a person. It depends on
`pdf-cli` for the fonts and the page drawing the window uses, and on
`pdf-session` for the open document, so every change it makes is planned,
proved and undoable in the same history -- an agent can do nothing a person
at the window could not, and is refused in the same words. It speaks MCP
(JSON-RPC over standard input and output) with a JSON reader of its own, and
no registry crate. Nothing depends on it.

The window depends on it for one thing only: `pdf_agent::connect`, which is
how the AI panel reaches a model. It never speaks MCP itself, and the desk
that holds an agent's open documents is not reached from UI code.

Where a line of text ends is `pdf-edit`'s `layout`, and nothing else's:
`breaks` answers where a line *may* end, `lines` where lines *do* end given
widths, and `around` (2026-09-20) what stands in the way -- a picture or a
drawing turned into one rectangle per line's band, which is how text flows
round an object without any part of the engine learning a second shape of
frame. The window draws such a frame from those same rows and that same rule
about which run a line takes, so what a person sees given up is what the text
gives up.

Cycles are architecture failures.
Rendering does not write PDFs. Semantics does not mutate atoms. The writer does
not infer user intent.

`pdf_semantics::MembershipAudit` and `pdf_semantics::DependencyIndex` both take
a membership as an argument rather than reading the index's own, so that a
deliberately wrong one can be passed in and the instrument shown to catch it
(`CONTRIBUTING.md`: a regression test must prove its measuring instrument on a known
answer). Both are pure functions of a graph and a list of objects; they own no
state, answer no question about selection, and plan and refuse nothing. The
dependency index follows soft masks, transparency groups and Type 3 procedures
to a bounded depth and says when it stopped, because an unfinished enumeration
must not be read as proof that nothing was found. It is page-local: a definition
shared with another page is invisible to it, and cross-page invalidation belongs
to the document layer.

`pdf_session::select` holds the two guarantees the design record makes about a
gesture by construction rather than by discipline: neither `CandidateStack` nor
`Gesture` has a method that could hit-test, so "select underneath" cycles a
frozen list and a drag cannot acquire a second object as the pointer crosses it.
An `ObjectRef` carries the revision it was resolved against; the window may not
name an object any other way.

### Enforcing boundaries

The checker uses Cargo metadata, including renamed, optional, target-specific,
build and test dependencies. An allowed dependency is optional in the policy:
removing it is fine, adding an unlisted edge requires documenting the reason
here and updating the policy in the same change. New crates need an explicit
entry. Local dependencies outside the workspace are refused, including a
dependency back into the old `panpdf` repository. External registry dependency
review remains governed by the dependency list in `Cargo.toml`.

The checker proves dependency declarations, not source-level API usage,
rendering fidelity, or thread safety. Its known-answer controls run first;
the Rust, Wasm, fuzz and corpus gates still have their separate jobs.

The write path reaches `pdf-security` directly. Encryption is a byte-resolution
layer in both directions: a stream read
out of a protected document is decrypted before its filters run, and a stream
written back into one must be encrypted with the same document key before it is
stored. A writer that knew only how to read would produce a file whose untouched
streams still decrypt and whose new one does not.

`pdf-session` is where a document stays open. It holds the edit history and,
beside it, the pages already interpreted from the current revision -- because
every question a viewer asks about a page is answered from one paint graph, and
before this layer existed each question rebuilt that graph from the document's
bytes. It owns the `History` rather than sitting beside one so that a caller
cannot apply a command without the pages it invalidated being forgotten.

What it may keep across an edit rests on the invariant above it: a plan states
the page it changes, and no pixel outside its declared effect moves. So a step
forgets that page and no other, and a step that names no page forgets all of
them.

`pdf_session::extract_pages` sits here for the same reason: a document made of
another document's pages is an empty document with pages imported into it and
the blank page it started from taken out, which is two commands on a session,
not a new way of writing a file. The layer that can apply commands is the layer
that can answer with the bytes they produced. `pdf_session::pictures_into_pdf`
is the same shape the other way round: a document whose pages are pictures is
blank pages the size of each picture with a picture placed on each, which is
commands on a session -- and the placing is the one that already proves the
page paints what the file holds.

Writing a *picture* file, by contrast, is `pdf_edit::png`, next to
`pdf_edit::image_file`, which reads one: what a JPEG or a PNG is belongs to one
place whichever direction it is going, and neither direction needs a document.
The renderer stays the layer that turns a page into pixels; nothing there knows
what a file of pixels looks like.

The names it hands out say which document *and* which revision they came from.
A revision is counted per open document, so every session begins at the same
number and two of them would otherwise agree on `r0` while meaning two unrelated
files. An `ObjectRef` therefore carries a `SessionId` that nothing outside this
crate can construct, and a reference from elsewhere is refused as `Foreign`
rather than as `Stale` -- the two are different mistakes, and only one of them
is worth retrying. Cloning a session yields a **new** document for the same
reason: the two share bytes only until either is edited, and a reference is a
promise about what an edit may still be committed against.

`pdf-app` is the editor. Its library decides what a pointer is on and what an
edit does to what is on screen, and is tested; its binary opens a window and
reads those answers. Nothing in the library touches a file, a process, a socket
or a clock, so `verify.sh` compiles it and every crate below it for
`wasm32-unknown-unknown` -- `eframe` has a web backend, and that check is what
keeps "build it natively first" from becoming "write it again".

`pdf-render` consumes the paint graph and produces pixels. It is also the first
consumer of every representation `pdf-paint` builds, which is why it exists
before the graph is complete: a representation with no consumer is a
representation whose errors are invisible.

## Permitted direct dependencies

Between the crates of this workspace, and no others. An edge removed needs no
change here; an edge added needs a reason above.

| Crate | May depend on |
| --- | --- |
| `pdf-bytes` | nothing |
| `pdf-syntax` | `pdf-bytes` |
| `pdf-security` | `pdf-bytes`, `pdf-syntax` |
| `pdf-font` | `pdf-bytes`, `pdf-syntax` |
| `pdf-content` | `pdf-bytes`, `pdf-font`, `pdf-security`, `pdf-syntax` |
| `pdf-paint` | `pdf-bytes`, `pdf-content`, `pdf-syntax` |
| `pdf-render` | `pdf-bytes`, `pdf-content`, `pdf-paint` |
| `pdf-semantics` | `pdf-bytes`, `pdf-font`, `pdf-paint`, `pdf-syntax` |
| `pdf-edit` | `pdf-bytes`, `pdf-content`, `pdf-paint`, `pdf-security`, `pdf-semantics`, `pdf-syntax` |
| `pdf-session` | `pdf-bytes`, `pdf-content`, `pdf-edit`, `pdf-paint`, `pdf-semantics`, `pdf-syntax` |
| `pdf-cli` | `pdf-bytes`, `pdf-content`, `pdf-edit`, `pdf-paint`, `pdf-render`, `pdf-semantics`, `pdf-session`, `pdf-syntax` |
| `pdf-ocr` | `pdf-bytes`, `pdf-content`, `pdf-edit`, `pdf-paint`, `pdf-render`, `pdf-session` |
| `pdf-compose` | nothing |
| `pdf-heap` | nothing |
| `pdf-print` | `pdf-bytes`, `pdf-content`, `pdf-render`, `pdf-session`, `pdf-syntax` |
| `pdf-app` | `pdf-agent`, `pdf-bytes`, `pdf-cli`, `pdf-content`, `pdf-edit`, `pdf-heap`, `pdf-ocr`, `pdf-paint`, `pdf-print`, `pdf-render`, `pdf-semantics`, `pdf-session`, `pdf-syntax` |
| `pdf-window` | `pdf-agent`, `pdf-app`, `pdf-bytes`, `pdf-cli`, `pdf-content`, `pdf-edit`, `pdf-heap`, `pdf-ocr`, `pdf-paint`, `pdf-print`, `pdf-render`, `pdf-semantics`, `pdf-session`, `pdf-syntax` |
| `pdf-desktop` | `pdf-app`, `pdf-bytes`, `pdf-cli`, `pdf-edit`, `pdf-semantics`, `pdf-session`, `pdf-window` |
| `pdf-agent` | `pdf-bytes`, `pdf-cli`, `pdf-content`, `pdf-edit`, `pdf-paint`, `pdf-render`, `pdf-semantics`, `pdf-session` |
