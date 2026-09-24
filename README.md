<div align="center">

<img src=".github/logo.png" width="110" alt="">

# PanPDF

**A free PDF editor that changes only what you edit.**

### [Try it free in your browser &rarr; panpdf.org](https://panpdf.org)

No account &middot; nothing uploaded &middot; the file stays on your computer

</div>

> [!WARNING]
> **Early development.** PanPDF edits real documents, but it is not finished:
> expect bugs, and things that change between versions. Keep a copy of files
> that matter, and [tell us what broke](../../issues/new).

![The editor with a page of text and shapes open](.github/screenshot.png)

## What it does

- **Retype text where it sits** -- the paragraph reflows in the document's own
  font, size and colour.
- **Leaves the rest alone** -- everything you did not touch keeps its exact bytes.
- **Any alphabet** -- Thai, Lao, Arabic, Hindi and the other scripts most
  editors break.
- **Pictures, pages and forms** -- move and turn pictures, reorder pages, fill
  in forms; protected files stay protected.
- **Straight answers** -- an edit it cannot make exactly is refused with a
  reason, never guessed.

## Get it

- **In the browser:** [panpdf.org](https://panpdf.org) -- on a computer;
  phones are not supported yet.
- **On the desktop:** Windows, Linux and macOS from the
  [latest release](../../releases/latest). The installers are not signed yet,
  so your system will warn you before it runs them.

## Build it yourself

Rust stable 1.97 or later. Nothing is linked at build time that is not in this
repository.

```sh
python3 fonts/vendor.py                 # the faces it draws a missing font from
cargo build --release -p pdf-desktop
./target/release/pdf-app document.pdf
```

`packaging/build.sh` makes the installable packages, and `./verify.sh` is the
gate: formatting, clippy, the tests and the browser target. On a minimal Debian
or Ubuntu install add `libgl1`, `libxkbcommon-x11-0` and `libwayland-client0`.
Optional: `tesseract-ocr` to read scanned pages, `zenity` for the system file
dialog.

## Contributing

[`CONTRIBUTING.md`](CONTRIBUTING.md), and security reports go to
[`SECURITY.md`](SECURITY.md). How the crates fit together is in
[`ARCHITECTURE.md`](ARCHITECTURE.md), which is also where the code's
explanation lives: the source itself carries no comments by design.

## Licence

GNU Affero General Public License, version 3 -- [`LICENSE`](LICENSE). If you
run a modified version for other people to use over a network, the AGPL asks
you to offer them its source.

The font files `fonts/vendor.py` fetches carry their own licences, recorded in
[`fonts/README.md`](fonts/README.md).
