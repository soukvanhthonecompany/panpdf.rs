pub mod blocks;
pub mod inlines;
pub mod laying_out;
pub mod tree;

pub use blocks::blocks;
pub use tree::{Align, Block, Inline, List, plain};

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Piece {
    pub text: String,
    pub bold: bool,
    pub italic: bool,
    pub code: bool,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Line {
    pub heading: u8,
    pub bullet: bool,
    pub depth: usize,
    pub quoted: bool,
    pub pieces: Vec<Piece>,
}

impl Line {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.pieces.iter().all(|piece| piece.text.trim().is_empty())
    }

    #[must_use]
    pub fn plain(&self) -> String {
        self.pieces
            .iter()
            .map(|piece| piece.text.as_str())
            .collect()
    }
}

#[must_use]
pub fn marked_up(text: &str) -> Vec<Line> {
    let mut out = Vec::new();
    flatten(&blocks(text), 0, &mut out);
    while out.last().is_some_and(Line::is_empty) {
        out.pop();
    }
    out
}

fn flatten(blocks: &[Block], depth: usize, out: &mut Vec<Line>) {
    for block in blocks {
        if !out.is_empty() {
            out.push(Line {
                depth,
                ..Line::default()
            });
        }
        one_block(block, depth, out);
    }
}

fn one_block(block: &Block, depth: usize, out: &mut Vec<Line>) {
    match block {
        Block::Heading { level, inlines } => out.push(Line {
            heading: *level,
            depth,
            pieces: pieces(inlines),
            bullet: false,
            quoted: false,
        }),
        Block::Paragraph { inlines } => rows(inlines, depth, out),
        Block::Code { text, .. } => {
            for line in text.lines() {
                out.push(Line {
                    depth,
                    pieces: vec![Piece {
                        text: line.to_owned(),
                        code: true,
                        ..Piece::default()
                    }],
                    ..Line::default()
                });
            }
        }
        Block::Break => out.push(Line {
            depth,
            pieces: vec![Piece {
                text: "\u{2014}".repeat(8),
                ..Piece::default()
            }],
            ..Line::default()
        }),
        Block::Quote { blocks } => {
            let before = out.len();
            flatten(blocks, depth + 1, out);
            for line in out.iter_mut().skip(before) {
                line.quoted = true;
            }
        }
        Block::List(list) => list_rows(list, depth, out),
        Block::Table {
            head, rows: body, ..
        } => {
            out.push(Line {
                depth,
                pieces: cells(head, true),
                ..Line::default()
            });
            for row in body {
                out.push(Line {
                    depth,
                    pieces: cells(row, false),
                    ..Line::default()
                });
            }
        }
    }
}

fn list_rows(list: &List, depth: usize, out: &mut Vec<Line>) {
    for (at, item) in list.items.iter().enumerate() {
        let mark = match list.first {
            None => "\u{2022}  ".to_owned(),
            Some(first) => format!("{}.  ", first + at as u64),
        };
        let before = out.len();
        for (which, block) in item.iter().enumerate() {
            if which > 0 {
                out.push(Line {
                    depth: depth + 1,
                    ..Line::default()
                });
            }
            one_block(block, depth + 1, out);
        }
        if let Some(line) = out.get_mut(before) {
            line.bullet = true;
            line.pieces.insert(
                0,
                Piece {
                    text: mark,
                    ..Piece::default()
                },
            );
        }
    }
}

fn cells(row: &[Vec<Inline>], head: bool) -> Vec<Piece> {
    let mut out = Vec::new();
    for (at, cell) in row.iter().enumerate() {
        if at > 0 {
            out.push(Piece {
                text: "   ".to_owned(),
                ..Piece::default()
            });
        }
        for mut piece in pieces(cell) {
            piece.bold |= head;
            out.push(piece);
        }
    }
    out
}

fn rows(inlines: &[Inline], depth: usize, out: &mut Vec<Line>) {
    let mut line = Line {
        depth,
        ..Line::default()
    };
    for inline in inlines {
        if matches!(inline, Inline::Hard) {
            out.push(std::mem::replace(
                &mut line,
                Line {
                    depth,
                    ..Line::default()
                },
            ));
            continue;
        }
        line.pieces.extend(pieces(std::slice::from_ref(inline)));
    }
    out.push(line);
}

pub(crate) fn pieces(inlines: &[Inline]) -> Vec<Piece> {
    let mut out = Vec::new();
    walk(inlines, &Piece::default(), &mut out);
    out
}

fn walk(inlines: &[Inline], so_far: &Piece, out: &mut Vec<Piece>) {
    for inline in inlines {
        match inline {
            Inline::Text(text) => out.push(Piece {
                text: text.clone(),
                ..so_far.clone()
            }),
            Inline::Soft => out.push(Piece {
                text: " ".to_owned(),
                ..so_far.clone()
            }),
            Inline::Hard => {}
            Inline::Code(text) => out.push(Piece {
                text: text.clone(),
                code: true,
                ..so_far.clone()
            }),
            Inline::Emphasis(inside) => walk(
                inside,
                &Piece {
                    italic: true,
                    ..so_far.clone()
                },
                out,
            ),
            Inline::Strong(inside) => walk(
                inside,
                &Piece {
                    bold: true,
                    ..so_far.clone()
                },
                out,
            ),
            Inline::Strike(inside)
            | Inline::Link { text: inside, .. }
            | Inline::Image { text: inside, .. } => walk(inside, so_far, out),
        }
    }
}

#[cfg(test)]
mod tests;
