use std::collections::BTreeMap;
use std::fmt::Write as _;

use pdf_agent::json::Json;
use pdf_agent::markup::tree::{Block, Inline, List};
use pdf_agent::markup::{blocks, plain};

fn main() {
    let mut args = std::env::args().skip(1);
    let Some(path) = args.next() else {
        eprintln!("usage: commonmark <spec.json> [--show <section>]");
        return;
    };
    let show = if args.next().as_deref() == Some("--show") {
        args.next().unwrap_or_default()
    } else {
        String::new()
    };
    let text = std::fs::read_to_string(&path).expect("the suite");
    let Json::List(examples) = Json::parse(&text).expect("the suite is JSON") else {
        eprintln!("the suite is a list of examples");
        return;
    };
    let mut tally: BTreeMap<String, (usize, usize)> = BTreeMap::new();
    for example in &examples {
        let (Some(markdown), Some(wanted), Some(section)) = (
            example.get("markdown").and_then(Json::as_str),
            example.get("html").and_then(Json::as_str),
            example.get("section").and_then(Json::as_str),
        ) else {
            continue;
        };
        let ours = html(&blocks(markdown));
        let row = tally.entry(section.to_owned()).or_default();
        row.1 += 1;
        if ours == wanted {
            row.0 += 1;
        } else if !show.is_empty() && section == show {
            println!("--- {markdown:?}\nwant: {wanted:?}\nours: {ours:?}\n");
        }
    }
    let (mut passed, mut all) = (0, 0);
    println!("| section | read | of |");
    println!("|---|---:|---:|");
    for (section, (right, count)) in &tally {
        passed += right;
        all += count;
        println!("| {section} | {right} | {count} |");
    }
    let share = f64::from(u32::try_from(passed).unwrap_or(0)) * 100.0
        / f64::from(u32::try_from(all).unwrap_or(1));
    println!("| **all** | **{passed}** | **{all}** |");
    println!("\n{share:.1}% of CommonMark 0.31.2");
}

fn html(blocks: &[Block]) -> String {
    let mut out = String::new();
    for block in blocks {
        match block {
            Block::Heading { level, inlines } => {
                let _ = writeln!(out, "<h{level}>{}</h{level}>", line(inlines));
            }
            Block::Paragraph { inlines } => {
                let _ = writeln!(out, "<p>{}</p>", line(inlines));
            }
            Block::Code { info, text } => {
                let tongue = info.split_whitespace().next().unwrap_or_default();
                let opening = if tongue.is_empty() {
                    "<code>".to_owned()
                } else {
                    format!("<code class=\"language-{}\">", escaped(tongue))
                };
                let _ = writeln!(out, "<pre>{opening}{}</code></pre>", escaped(text));
            }
            Block::Break => out.push_str("<hr />\n"),
            Block::Quote { blocks } => {
                let _ = writeln!(out, "<blockquote>\n{}</blockquote>", html(blocks));
            }
            Block::List(list) => out.push_str(&listed(list)),
            Block::Table { .. } => {}
        }
    }
    out
}

fn listed(list: &List) -> String {
    let mut out = String::new();
    match list.first {
        None => out.push_str("<ul>\n"),
        Some(1) => out.push_str("<ol>\n"),
        Some(first) => {
            let _ = writeln!(out, "<ol start=\"{first}\">");
        }
    }
    for item in &list.items {
        if item.is_empty() {
            out.push_str("<li></li>\n");
            continue;
        }
        let inside = if list.loose {
            format!("\n{}", html(item))
        } else {
            tight(item)
        };
        let _ = writeln!(out, "<li>{inside}</li>");
    }
    out.push_str(if list.first.is_some() {
        "</ol>\n"
    } else {
        "</ul>\n"
    });
    out
}

fn tight(item: &[Block]) -> String {
    let mut out = String::new();
    for (at, block) in item.iter().enumerate() {
        match block {
            Block::Paragraph { inlines } => {
                if at > 0 {
                    out.push('\n');
                }
                out.push_str(&line(inlines));
            }
            other => {
                if at > 0 && !out.ends_with('\n') {
                    out.push('\n');
                }
                out.push_str(&html(std::slice::from_ref(other)));
            }
        }
    }
    out
}

fn line(inlines: &[Inline]) -> String {
    let mut out = String::new();
    for inline in inlines {
        match inline {
            Inline::Text(text) => out.push_str(&escaped(text)),
            Inline::Soft => out.push('\n'),
            Inline::Hard => out.push_str("<br />\n"),
            Inline::Code(text) => {
                let _ = write!(out, "<code>{}</code>", escaped(text));
            }
            Inline::Emphasis(inside) => {
                let _ = write!(out, "<em>{}</em>", line(inside));
            }
            Inline::Strong(inside) => {
                let _ = write!(out, "<strong>{}</strong>", line(inside));
            }
            Inline::Strike(inside) => {
                let _ = write!(out, "<del>{}</del>", line(inside));
            }
            Inline::Link { to, text } => {
                let _ = write!(out, "<a href=\"{}\">{}</a>", address(to), line(text));
            }
            Inline::Image { at, text } => {
                let _ = write!(
                    out,
                    "<img src=\"{}\" alt=\"{}\" />",
                    address(at),
                    escaped(&plain(text))
                );
            }
        }
    }
    out
}

fn escaped(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn address(to: &str) -> String {
    let mut out = String::new();
    for byte in to.bytes() {
        if byte.is_ascii_alphanumeric() || b"-_.~!*'();:@&=+$,/?#[]%".contains(&byte) {
            out.push(char::from(byte));
        } else {
            let _ = write!(out, "%{byte:02X}");
        }
    }
    escaped(&out)
}
