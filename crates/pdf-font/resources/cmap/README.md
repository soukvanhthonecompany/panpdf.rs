# Predefined `CMap` resources

A `CMap` named by `/Encoding` in a Type0 font dictionary rather than embedded in
the document is *predefined*: PDF 32000-1 9.7.5.2 makes it a resource the reader
is expected to hold, and a reader that does not hold it cannot map that font's
character codes to CIDs at all. These files are those resources.

Nothing here is a font. A `CMap` maps character codes to CIDs; which glyph a CID
selects is the descendant `CIDFont`'s question and is answered elsewhere.

## What is here, and why only this

| File | Selected by | Corpus evidence |
| --- | --- | --- |
| `Adobe-Korea1/UniKS-UTF16-H` | 475 composite fonts in 7 documents | 470 pages that would not render |
| `Adobe-GB1/GBK-EUC-UCS2` | `pdf_font::legacy_cjk`, for a font whose `/BaseFont` is GBK bytes and whose declared encoding contradicts it | 1,035 glyph entries of a `pdf.js` test-suite document that drew nothing |

The second file is **not** selected by a document's `/Encoding`. It is a
recovery table: a family of Chinese producers writes a simple `/TrueType` font
whose name is the GBK bytes of a Chinese family, marks it symbolic, declares
`/WinAnsiEncoding`, embeds no program, and then shows GBK double-byte text
through it. Every statement in that dictionary except the name is false, and
this table is what turns those bytes into the characters the name says they
are. `pdf_font::legacy_cjk` documents the condition and how it differs from
pdf.js's repair of the same files.

`cargo run --release --example cmaps -p pdf-cli -- documents --all --summary`
counts every `/Encoding` the corpus selects. It selects exactly two names:
`/Identity-H`, which needs no resource, and `/UniKS-UTF16-H`; the GBK table
above is reached by a repair rather than by a name. Shipping the rest of
Adobe's collection would be several megabytes of data no measurement asks for,
and `pdf_font::predefined` refuses an unshipped name by name rather than
approximating it, so a document that needs another one says so instead of
drawing the wrong glyphs.

Adding one is copying its file here and adding a row to `RESOURCES` in
`crates/pdf-font/src/predefined.rs`. A resource that inherits through `usecmap`
also needs the file it names.

## Provenance

### `Adobe-GB1/GBK-EUC-UCS2`

- **Origin:** Adobe's CMap resources for the Adobe-GB1 character collection.
  The file carries its own `%%Version: 4.006` and `%%Copyright: Copyright
  1990-2019 Adobe. All rights reserved.` header, kept byte for byte.
- **Obtained from:** Debian/Ubuntu package `poppler-data` 0.4.12-1, path
  `/usr/share/poppler/cMap/Adobe-GB1/GBK-EUC-UCS2`.
- **SHA-256:** `25b3158ed0bad3fe9529873c0108e2cac9ade4e4824860568dc1deea1ddb9916`
- **Licence:** BSD-3-Clause, as printed in the file's own `%%Copyright`
  comments. The notice is retained by shipping the file unmodified.
- **Shipped or test-only:** **shipped.** It is compiled into `pdf-font` with
  `include_bytes!` and parsed on the first run that needs it.
- **Checked against a known answer:** yes, in `pdf_font::legacy_cjk`'s tests.
  The three code-space ranges the decoder's byte tests are written from are
  read back out of the file itself, so a resource swapped for one with a
  different code space fails rather than silently re-segmenting every Chinese
  page; `<8140>` decodes to the U+4E02 the file names; the family name
  `CB CE CC E5` decodes to 宋体; and the 126 codes whose trailing byte is
  `0x7F` -- the one hole the declared space admits and GBK never uses -- stay
  unresolved rather than drawing U+0000.

### `Adobe-Korea1/UniKS-UTF16-H`

- **Origin:** Adobe's CMap resources for the Adobe-Korea1 character collection.
  The file carries its own `%%Version: 1.008` and `%%Copyright: Copyright
  1990-2019 Adobe. All rights reserved.` header, kept byte for byte.
- **Obtained from:** Debian/Ubuntu package `poppler-data` 0.4.12-1, path
  `/usr/share/poppler/cMap/Adobe-Korea1/UniKS-UTF16-H`. `/var/lib/ghostscript/
  CMap/UniKS-UTF16-H` is a symbolic link to the same file, so that is one
  source and not two.
- **SHA-256:** `0a1dc38d8dd4e1f55b5a3ab978a2b091109f88f4d08587bbb7864370cda8cfd5`
- **Licence:** BSD-3-Clause, as printed in the file's own `%%Copyright`
  comments. The notice is retained by shipping the file unmodified.
- **Shipped or test-only:** **shipped.** It is compiled into `pdf-font` with
  `include_bytes!` and is read whenever a document selects it.
- **Checked against an independent implementation:** yes.
  `probes/cmap/compare.py` reads this file and PDFium's compiled
  `core/fpdfapi/cmaps/Korea1` tables and compares all 65,536 two-byte codes.
  **No code is mapped to two different CIDs.** Four codes -- `<00a0>`,
  `<00b7>`, `<fa2e>`, `<fa2f>` -- are mapped by this file and left at notdef by
  PDFium, whose tables are generated from an older supplement. That is a
  version difference in PDFium's favour of nothing: it draws nothing for those
  four codes and this reads a CID for them.
