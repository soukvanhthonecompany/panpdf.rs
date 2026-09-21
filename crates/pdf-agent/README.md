# panpdf-mcp — PanPDF for AI agents

`panpdf-mcp` gives an AI agent — Claude, or any program that speaks the
[Model Context Protocol](https://modelcontextprotocol.io) — the same PDF engine
as the PanPDF window. An agent can open a PDF, read it exactly, see a page as a
picture, and change it: rewrite text in its own font, write new text in any
script the document can carry, fill a form, set the document's properties,
add, delete, move and turn pages, undo, and save.

Every change goes through the engine's proved plans, the same ones the window
uses. The agent can do nothing to a file that a person at the window could not,
and a refusal says why in the same words.

## Build

```sh
cargo build --release -p pdf-agent
# the program is target/release/panpdf-mcp
```

## Connect

**Claude Code**

```sh
claude mcp add panpdf -- /full/path/to/target/release/panpdf-mcp
```

**Claude Desktop** and other MCP clients: add this to the client's MCP
configuration (for Claude Desktop, `claude_desktop_config.json`):

```json
{
  "mcpServers": {
    "panpdf": { "command": "/full/path/to/target/release/panpdf-mcp" }
  }
}
```

The server speaks MCP over standard input and output. It answers both the
current protocol revision (2026-07-28, no handshake) and the older revisions
that open with `initialize` (2025-11-25, 2025-06-18, 2025-03-26 and 2024-11-05).

## What an agent can ask for

| Tool | What it does |
| --- | --- |
| `open_document` | Opens a PDF and returns a handle (`doc-1`) that the other tools take |
| `list_documents`, `close_document` | What is open; close one |
| `document_info` | Title, author and the other properties, page sizes, bookmarks, form fields, signatures |
| `read_text` | Text in blocks (`p3-b12`), each with its box on the page in points and its size |
| `find_text` | Every block that holds a piece of text, in the whole document or a range of pages; at most 200 blocks, each cut to 600 characters |
| `render_page` | A page as a PNG picture, for layout, pictures and scanned pages |
| `replace_text` | Replaces a block's text, or only the words passed as `find`; the rest keeps its style |
| `add_text` | New text in a frame, in a font that has every character of it |
| `list_fonts` | The font families new text can be set in |
| `set_properties` | Title, author, subject, keywords |
| `fill_field` | Fills a form field |
| `add_blank_page`, `delete_pages`, `move_pages`, `rotate_pages`, `insert_pages` | Pages. `insert_pages` takes them from another PDF, which must not be password-protected |
| `undo`, `redo` | Walks the edits back and forward |
| `save_document` | Writes the result. Without a path it is saved beside the original as `…-edited.pdf` |

Summarising, translating or deciding what to change is the agent's work. The
tools give it exact text and pictures to think about, and a safe way to make
the change.

## Safety

- Opening a document changes nothing on disk. Edits stay in memory until
  `save_document`.
- `save_document` never writes over an existing file unless `replace: true` is
  passed. It never writes over the original if someone changed it on disk since
  it was opened. It writes to a temporary file first and renames it, so a failed
  save leaves whatever was there.
- A document whose author restricted editing is read-only unless it was opened
  with `set_aside_restrictions`, which the tool's description tells the agent to
  use only on the person's word. A password is never guessed.
- `document_info` names every signature, so the agent can tell the person before
  editing a signed document.

## Positions

A block name (`p3-b12`) holds a page number, so it is only good while the
pages stand as they were: once pages are added, removed, moved or inserted
-- or an undo or a redo does one of those -- every name from before is
refused, and the answer says to read the page again. That is what stops a
footer or a letterhead every page shares being rewritten on the wrong page.

Pages are counted from 1. Positions are in points (1/72 inch) from the
**top-left corner of the page as shown**, x to the right and y down, with the
page's crop and rotation already applied. That matches a picture from
`render_page`.
