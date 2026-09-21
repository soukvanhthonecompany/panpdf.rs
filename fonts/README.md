# The packaged substitution faces

A PDF may show text in a font it never embeds. This directory is the set of
faces this project will draw such a run from when it is asked for a *reproducible*
answer — `docs/design/RENDER-REPAIR-BATCH.md` F2's "a clean machine with no
system fonts must draw the substitution fixtures".

Nothing here is downloaded while a document is open. `vendor.py` is an explicit
step, and a renderer that does not find `packaged/` says so and searches the
host instead.

```sh
python3 fonts/vendor.py           # download, verify, unpack into packaged/
python3 fonts/vendor.py --check   # verify what is already unpacked
PANPDF_FONTS=packaged cargo run --release -p pdf-cli -- render-page file.pdf out.ppm
```

## What is here, and why only this

Chosen from what the corpus measurably asks for — `--example fontcensus` over
58 sampled documents — not for completeness. Every file is named with its
SHA-256 in [`manifest.json`](manifest.json), which is what makes two machines
comparable; the licence notice of every source is unpacked beside the faces it
belongs to.

Read the census carefully before quoting it: its **family view counts every
resource**, embedded ones included, and the Lao names at the top of that list
are fonts these documents *carry*. The families actually **substituted** in
that sample are Latin — Times 98 usages, TimesNewRomanPSMT 17, Courier 10,
Helvetica 10, and single figures of Verdana, Tahoma, Calibri and
LucidaTypewriter — which is why Liberation is the largest part of this package
by a wide margin. The Lao and Thai faces are here for the *missing*-font case
in the same corpus rather than for a count of it.

| Faces | Answers | Source | Licence |
| --- | --- | --- | --- |
| Liberation Serif / Sans / Mono, 4 styles each | The Standard 14, and the Arial / Times New Roman / Courier New the corpus names 500+ times | [liberation-fonts 2.1.5](https://github.com/liberationfonts/liberation-fonts) | SIL OFL 1.1 |
| Noto Sans Lao, Noto Serif Lao, regular and bold | Lao — the script this corpus is written in, and the one whose *missing*-font case the engine already has a named path for: seven Lao textbooks select `UniKS-UTF16-H` for a Lao font, embed no program and carry no `/ToUnicode`, so their codes are the only statement of what they say | [notofonts/lao 2.003](https://github.com/notofonts/lao) | SIL OFL 1.1 |
| Noto Sans Thai, regular and bold | Thai, for the same reason and at a tenth the size | [notofonts/thai 2.002](https://github.com/notofonts/thai) | SIL OFL 1.1 |
| DejaVu Sans, regular and bold | Greek, Cyrillic, punctuation, arrows and the symbol codes a Standard 14 `Symbol` run reaches through its own encoding | [dejavu-fonts 2.37](https://github.com/dejavu-fonts/dejavu-fonts) | Bitstream Vera and Arev, both permissive |
| Saysettha OT 2.000, regular | Not a substitute but a **reference**. The Lao textbooks in this corpus embed subsets of Saysettha OT with no `/ToUnicode` for their letters, named `g302` and so on. `pdf_font::outline_match` reads each unread glyph as the character whose Saysettha OT glyph draws the same outline: 1,832 of 1,884 glyphs on one sample page, and none against Noto Sans Lao, which is the control. Added 2026-09-14 at the owner's request | [laoscript.net](https://laoscript.net/download/) | SIL OFL 1.1, Reserved Font Name Saysettha; copyright John M. Durdin, in the face's `name` table |

## What is deliberately **not** here

- **CJK.** `Noto Sans CJK` is about 20 MB per face and its collection is over a
  hundred; `Droid Sans Fallback` is 4.5 MB. The corpus's Chinese is two
  documents, both recovered through `pdf_font::legacy_cjk`, and a host that has
  a CJK face draws them today under `PANPDF_FONTS=packaged+system`. Adding one
  is adding a row to the manifest and rerunning `vendor.py`; the decision to
  wait is about the megabyte cost of the default package, and it is recorded
  here rather than left to be discovered by a page that does not draw.
- **Symbol and ZapfDingbats faces.** The two Standard 14 symbolic faces have no
  metric-compatible replacement under a licence this project can redistribute.
  Their codes are read through the documented Adobe encodings instead — code
  0xB7 is `bullet`, which is U+2022 — and drawn from DejaVu Sans, which carries
  those characters. A run whose code reaches a character DejaVu does not carry
  is reported unmapped.
- **Metric substitution.** MuPDF scales every substituted glyph to the declared
  width. This build draws the substitute's own outline at the document's own
  origin and reports the run as substituted; see
  `docs/research/font-substitution-upstream.md`.

## Order

`manifest.json`'s `faces` list is the **resolution order**, not a description
of a directory: two packaged faces that answer the same family and style
resolve to the earlier entry, on every machine.
