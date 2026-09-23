use super::{Kind, Paragraph, Part, mathematics_as_text, paragraphs, parts};

#[test]
fn a_written_document_becomes_paragraphs() {
    let written = "# Investing in 2026\n\
                   \n\
                   The year ahead looks mixed.\n\
                   \n\
                   ## Where the money goes\n\
                   - Bonds\n\
                   - Gold\n";
    let out = paragraphs(written, 11.0);
    assert_eq!(out.len(), 5, "{out:#?}");
    assert_eq!(
        out[0],
        Paragraph {
            text: "Investing in 2026".to_owned(),
            size: 19.8,
            bold: true,
            italic: false,
            space_before: 0.0,
            indent: 0.0,
            kind: Kind::Heading(1),
            bullet: false,
        }
    );
    assert!((out[1].size - 11.0).abs() < 0.001);
    assert!(!out[1].bold);
    assert!((out[1].space_before - 4.95).abs() < 0.001);
    assert!((out[2].size - 15.4).abs() < 0.001);
    assert!(out[2].bold);
    assert!(
        out[2].space_before > out[1].space_before,
        "a heading is pushed away from what came before it"
    );
    assert_eq!(out[3].text, "Bonds");
    assert!(out[3].bullet);
    assert!((out[3].indent - 15.4).abs() < 0.001);
    assert_eq!(out[4].text, "Gold");

    let numbered = paragraphs("1. one\n2. two\n", 10.0);
    assert_eq!(
        numbered[1].text, "2.  two",
        "a number is written, not drawn"
    );
    assert!(!numbered[1].bullet);
}

#[test]
fn a_paragraph_is_written_in_one_style() {
    let out = paragraphs("**All of it matters**", 12.0);
    assert!(out[0].bold);
    assert_eq!(out[0].text, "All of it matters");

    let out = paragraphs("the **important** part", 12.0);
    assert!(!out[0].bold, "one bold word does not make a bold paragraph");
    assert_eq!(out[0].text, "the important part");
}

#[test]
fn nothing_written_places_nothing() {
    assert!(paragraphs("", 12.0).is_empty());
    assert!(paragraphs("\n\n   \n", 12.0).is_empty());
}

#[test]
fn one_number_scales_the_document() {
    let small = paragraphs("# Title\n\nBody\n", 10.0);
    let big = paragraphs("# Title\n\nBody\n", 20.0);
    assert!((big[0].size / small[0].size - 2.0).abs() < 0.001);
    assert!((big[1].space_before / small[1].space_before - 2.0).abs() < 0.001);
}

#[test]
fn a_page_gets_tables_rules_code_charts_and_quotes() {
    let written = "| Name | Answer |\n|---|---|\n| Tom | A much longer answer |\n\n---\n\n```\nlet a = 1;\nlet b = 2;\n```\n\n```chart\n{\"type\":\"bar\"}\n```\n\n> Careful.\n";
    let out = parts(written, 10.0);
    assert_eq!(out.len(), 5, "{out:#?}");
    let Part::Table(table) = &out[0] else {
        panic!("a table: {out:#?}");
    };
    assert!(table.head.iter().all(|cell| cell.bold));
    assert!(table.rows[0].iter().all(|cell| !cell.bold));
    assert_eq!(table.rows[0][1].text, "A much longer answer");
    assert!((table.columns[0] - 4.0 / 24.0).abs() < 0.001);
    assert!((table.columns.iter().sum::<f32>() - 1.0).abs() < 0.001);
    assert!(matches!(out[1], Part::Rule { .. }));
    let Part::Text(code) = &out[2] else {
        panic!("code is a paragraph");
    };
    assert_eq!(code.text, "let a = 1;\nlet b = 2;");
    assert_eq!(code.kind, Kind::Code);
    assert!((code.size - 9.0).abs() < 0.001);
    let Part::Chart { spec, .. } = &out[3] else {
        panic!("a chart");
    };
    assert_eq!(spec, "{\"type\":\"bar\"}");
    let Part::Text(quote) = &out[4] else {
        panic!("a quote");
    };
    assert_eq!(quote.kind, Kind::Quote);

    let tight = paragraphs("- one\n- two\n", 10.0);
    assert!((tight[1].space_before - 1.5).abs() < 0.001);
    let loose = paragraphs("- one\n\n- two\n", 10.0);
    assert!((loose[1].space_before - 4.5).abs() < 0.001);
}

#[test]
fn mathematics_is_held_out_of_the_markdown() {
    let out = paragraphs("Let $a_1 + b_2$ be small.", 11.0);
    assert_eq!(out[0].text, "Let a₁ + b₂ be small.");
    let out = paragraphs("$$\\begin{pmatrix} 1 & 2 \\\\ 3 & 4 \\end{pmatrix}$$", 11.0);
    assert_eq!(out[0].kind, Kind::Math);
    assert_eq!(
        out[0].text,
        "\\begin{pmatrix} 1 & 2 \\\\ 3 & 4 \\end{pmatrix}"
    );
    let out = paragraphs("$$\n\\frac{1}{2}\n$$\n", 11.0);
    assert_eq!(out[0].kind, Kind::Math);
    assert_eq!(out[0].text, "\\frac{1}{2}");
    let out = paragraphs("It costs $5 and $10 today.", 11.0);
    assert_eq!(out[0].text, "It costs $5 and $10 today.");
    let out = paragraphs("Write `$x$` to get it.", 11.0);
    assert_eq!(out[0].text, "Write $x$ to get it.");

    let bare: Vec<Paragraph> = crate::markup::blocks("Let $a_1 + b_2$ be small.")
        .iter()
        .filter_map(|block| match block {
            crate::markup::Block::Paragraph { inlines } => Some(Paragraph {
                text: crate::markup::plain(inlines),
                size: 11.0,
                bold: false,
                italic: false,
                space_before: 0.0,
                indent: 0.0,
                kind: Kind::Body,
                bullet: false,
            }),
            _ => None,
        })
        .collect();
    assert_ne!(
        bare[0].text,
        out_text(),
        "the control: unheld, it is mangled"
    );
}

fn out_text() -> String {
    "Let a₁ + b₂ be small.".to_owned()
}

#[test]
fn the_chat_reads_mathematics_as_unicode() {
    let chat = mathematics_as_text("Take $\\int_0^1 x^2 dx$ now.");
    let read = crate::markup::marked_up(&chat);
    assert_eq!(read[0].plain(), "Take ∫₀¹ x² dx now.");
    let chat = mathematics_as_text("On $[a](b)$ it holds.");
    let read = crate::markup::marked_up(&chat);
    assert_eq!(read[0].plain(), "On [a](b) it holds.");
    assert_eq!(
        mathematics_as_text("It costs $5 and $10."),
        "It costs $5 and $10."
    );
    let unescaped = "On [a](b) it holds.";
    assert_eq!(
        crate::markup::marked_up(unescaped)[0].plain(),
        "On a it holds.",
        "the control: unescaped, it is a link"
    );
}
