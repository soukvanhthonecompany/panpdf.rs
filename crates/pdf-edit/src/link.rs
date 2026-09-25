use pdf_bytes::ByteStore;
use pdf_syntax::Reference;

use crate::field_settings::pdf_literal;
use crate::fill_field::set_entries;
use crate::plan::{Capability, Effect, Plan, PlannedBody, PlannedWrite};
use crate::spike_move_text::{PlannerPage, SpikeError};

const SMALLEST: f64 = 3.0;
const LONGEST_NAME: usize = 1_024;

const MOST_PAGES: usize = 1_000_000;

const MOST_AT_ONCE: usize = 500;

const LONGEST_ADDRESS: usize = 4_096;

#[derive(Clone, Debug, PartialEq)]
pub enum Target {
    Page(usize, Arrival),
    Address(String),
    Name(String),
    Document {
        file: String,
        page: usize,
        arrival: Arrival,
    },
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Arrival {
    #[default]
    InheritZoom,
    FitPage,
    FitWidth,
    FitHeight,
    FitVisible,
    ActualSize,
    Percent(f64),
}

impl Arrival {
    const fn takes_a_corner(self) -> bool {
        matches!(
            self,
            Self::FitWidth | Self::FitHeight | Self::ActualSize | Self::Percent(_)
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Look {
    pub width: f64,
    pub style: LinkBorder,
    pub colour: Option<[f64; 3]>,
    pub highlight: Highlight,
}

impl Default for Look {
    fn default() -> Self {
        Self {
            width: 0.0,
            style: LinkBorder::Solid,
            colour: None,
            highlight: Highlight::Invert,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum LinkBorder {
    #[default]
    Solid,
    Dashed,
    Underline,
}

impl LinkBorder {
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Solid => "S",
            Self::Dashed => "D",
            Self::Underline => "U",
        }
    }

    fn of_name(name: &str) -> Option<Self> {
        match name {
            "S" => Some(Self::Solid),
            "D" => Some(Self::Dashed),
            "U" => Some(Self::Underline),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Highlight {
    None,
    #[default]
    Invert,
    Outline,
    Inset,
}

impl Highlight {
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::None => "N",
            Self::Invert => "I",
            Self::Outline => "O",
            Self::Inset => "P",
        }
    }

    fn of_name(name: &str) -> Option<Self> {
        match name {
            "N" => Some(Self::None),
            "I" => Some(Self::Invert),
            "O" => Some(Self::Outline),
            "P" => Some(Self::Inset),
            _ => None,
        }
    }
}

fn refused(reason: &'static str) -> SpikeError {
    SpikeError::RetypeUnsupported(reason)
}

fn entries_of(
    (source, credential): (&ByteStore, &[u8]),
    target: &Target,
    pages: &[Reference],
) -> Result<Vec<(&'static [u8], String)>, SpikeError> {
    match target {
        Target::Page(page, arrival) => {
            let reference = pages
                .get(*page)
                .ok_or_else(|| refused("this document has no such page"))?;
            Ok(vec![
                (
                    b"/Dest".as_slice(),
                    format!(
                        "[{} {}]",
                        crate::object_edit::reference_text(*reference),
                        destination_view((source, credential), *reference, *arrival)?
                    ),
                ),
                (b"/A", String::new()),
            ])
        }
        Target::Document {
            file,
            page,
            arrival,
        } => {
            let file = file.trim();
            if file.is_empty() || file.len() > LONGEST_NAME || file.chars().any(char::is_control) {
                return Err(refused("a file's name is one line of text"));
            }
            if *page > MOST_PAGES {
                return Err(refused("this page number is not one"));
            }
            Ok(vec![
                (
                    b"/A".as_slice(),
                    format!(
                        "<< /S /GoToR /F {} /D [{page} {}] /NewWindow false >>",
                        pdf_literal(file),
                        remote_view(*arrival)?
                    ),
                ),
                (b"/Dest", String::new()),
            ])
        }
        Target::Name(name) => {
            let name = name.trim();
            if name.is_empty() || name.len() > LONGEST_NAME || name.chars().any(char::is_control) {
                return Err(refused("a destination's name is one line of text"));
            }
            if !named_here((source, credential))
                .iter()
                .any(|held| held == name)
            {
                return Err(refused("this document does not name that destination"));
            }
            Ok(vec![
                (b"/Dest".as_slice(), pdf_literal(name)),
                (b"/A", String::new()),
            ])
        }
        Target::Address(address) => {
            let address = address.trim();
            if address.is_empty()
                || address.len() > LONGEST_ADDRESS
                || address.chars().any(char::is_control)
            {
                return Err(refused("a link's address is one line of text"));
            }
            Ok(vec![
                (
                    b"/A".as_slice(),
                    format!("<< /S /URI /URI {} >>", pdf_literal(address)),
                ),
                (b"/Dest", String::new()),
            ])
        }
    }
}

pub(crate) fn destination_view(
    (source, credential): (&ByteStore, &[u8]),
    page: Reference,
    arrival: Arrival,
) -> Result<String, SpikeError> {
    let corner = if arrival.takes_a_corner() {
        top_left_of((source, credential), page)
    } else {
        None
    };
    let (left, top) = match corner {
        Some((left, top)) => (trimmed(left), trimmed(top)),
        None => ("null".to_owned(), "null".to_owned()),
    };
    Ok(match arrival {
        Arrival::InheritZoom => "/XYZ null null null".to_owned(),
        Arrival::FitPage => "/Fit".to_owned(),
        Arrival::FitWidth => format!("/FitH {top}"),
        Arrival::FitHeight => format!("/FitV {left}"),
        Arrival::FitVisible => "/FitB".to_owned(),
        Arrival::ActualSize => format!("/XYZ {left} {top} 1"),
        Arrival::Percent(percent) => {
            if !percent.is_finite() || !(1.0..=6400.0).contains(&percent) {
                return Err(refused("a zoom is between one and six thousand per cent"));
            }
            format!("/XYZ {left} {top} {}", trimmed(percent / 100.0))
        }
    })
}

fn top_left_of((source, credential): (&ByteStore, &[u8]), page: Reference) -> Option<(f64, f64)> {
    let reader = crate::form::Reader::open(source, credential).ok()?;
    let node = reader.at(page)?;
    let found = reader.inherited(&node, b"/MediaBox")?;
    let pdf_syntax::ObjectKind::Array(items) = found.value.kind() else {
        return None;
    };
    let edges: Vec<f64> = items
        .iter()
        .map(|item| {
            reader
                .follow(&found, item)
                .and_then(|edge| crate::form::Reader::number(&edge))
        })
        .collect::<Option<Vec<f64>>>()?;
    let [x0, y0, x1, y1] = <[f64; 4]>::try_from(edges).ok()?;
    if ![x0, y0, x1, y1].iter().all(|edge| edge.is_finite()) {
        return None;
    }
    Some((x0.min(x1), y0.max(y1)))
}

fn remote_view(arrival: Arrival) -> Result<String, SpikeError> {
    Ok(match arrival {
        Arrival::InheritZoom => "/XYZ null null null".to_owned(),
        Arrival::FitPage => "/Fit".to_owned(),
        Arrival::FitWidth => "/FitH null".to_owned(),
        Arrival::FitHeight => "/FitV null".to_owned(),
        Arrival::FitVisible => "/FitB".to_owned(),
        Arrival::ActualSize => "/XYZ null null 1".to_owned(),
        Arrival::Percent(percent) => {
            if !percent.is_finite() || !(1.0..=6400.0).contains(&percent) {
                return Err(refused("a zoom is between one and six thousand per cent"));
            }
            format!("/XYZ null null {}", trimmed(percent / 100.0))
        }
    })
}

fn checked_box(rect: [f64; 4]) -> Result<[f64; 4], SpikeError> {
    let [x0, y0, x1, y1] = rect;
    if ![x0, y0, x1, y1].iter().all(|edge| edge.is_finite()) {
        return Err(refused("a link's box is four numbers"));
    }
    let rect = [x0.min(x1), y0.min(y1), x0.max(x1), y0.max(y1)];
    if rect[2] - rect[0] < SMALLEST || rect[3] - rect[1] < SMALLEST {
        return Err(refused("this box is too small to click"));
    }
    Ok(rect)
}

fn pages_of((source, credential): (&ByteStore, &[u8])) -> Result<Vec<Reference>, SpikeError> {
    pdf_content::page_references_with_password(
        source,
        pdf_content::PageContentLimits::default(),
        credential,
    )
    .map_err(|_| refused("this document's pages cannot be walked"))
}

fn plan_of(
    page: &PlannerPage<'_>,
    page_index: usize,
    writes: Vec<PlannedWrite>,
    region: [f64; 4],
) -> Plan {
    let target = page
        .program
        .streams
        .first()
        .map_or(page.program.page, |stream| stream.reference);
    Plan::new(
        Capability::Exact,
        writes,
        Effect {
            page_index,
            moved: Vec::new(),
            target_stream: target,
            declared_region: Some(region),
        },
    )
}

pub(crate) fn plan_add_link(
    source: &ByteStore,
    page: PlannerPage<'_>,
    page_index: usize,
    (rect, target, look): ([f64; 4], &Target, Look),
) -> Result<Plan, SpikeError> {
    plan_add_links(source, page, page_index, &[(rect, target.clone(), look)])
}

pub(crate) fn plan_add_links(
    source: &ByteStore,
    page: PlannerPage<'_>,
    page_index: usize,
    wanted: &[([f64; 4], Target, Look)],
) -> Result<Plan, SpikeError> {
    let credential = page.credential;
    if wanted.is_empty() {
        return Err(refused("there is nothing here to link"));
    }
    if wanted.len() > MOST_AT_ONCE {
        return Err(refused("this is more links than one page is given at once"));
    }
    let pages = pages_of((source, credential))?;
    let first = crate::block_rewrite::next_object_number(source)?;
    let mut writes = Vec::new();
    let mut made = Vec::new();
    let mut region: Option<[f64; 4]> = None;
    for (step, (rect, target, look)) in wanted.iter().enumerate() {
        let rect = checked_box(*rect)?;
        if !look.width.is_finite() || !(0.0..=100.0).contains(&look.width) {
            return Err(refused(
                "a link's border is between no points and a hundred",
            ));
        }
        let mut entries = entries_of((source, credential), target, &pages)?;
        entries.extend(look_entries(*look));
        let written: Vec<String> = entries
            .iter()
            .filter(|(_, value)| !value.is_empty())
            .map(|(key, value)| format!("{} {value}", String::from_utf8_lossy(key)))
            .collect();
        #[expect(
            clippy::cast_possible_truncation,
            reason = "one object number per link, and there are at most a few hundred"
        )]
        let link = Reference::new(first + step as u32, 0);
        writes.push(PlannedWrite {
            reference: link,
            body: PlannedBody::Direct {
                body: format!(
                    "<< /Type /Annot /Subtype /Link /Rect [{} {} {} {}] /F 4 {} >>",
                    trimmed(rect[0]),
                    trimmed(rect[1]),
                    trimmed(rect[2]),
                    trimmed(rect[3]),
                    written.join(" ")
                )
                .into_bytes(),
            },
        });
        made.push((link, rect, target, *look));
        region = Some(match region {
            Some(one) => [
                one[0].min(rect[0]),
                one[1].min(rect[1]),
                one[2].max(rect[2]),
                one[3].max(rect[3]),
            ],
            None => rect,
        });
    }
    let references: Vec<Reference> = made.iter().map(|(link, ..)| *link).collect();
    writes.extend(crate::new_field::listed_on_page(
        (source, credential),
        page.program.page,
        &references,
    )?);
    for (link, rect, target, look) in &made {
        prove_link(
            (source, credential),
            &writes,
            (page_index, *link),
            (*rect, target),
        )?;
        prove_look((source, credential), &writes, (page_index, *link), *look)?;
    }
    let region = region.ok_or_else(|| refused("there is nothing here to link"))?;
    Ok(plan_of(&page, page_index, writes, region))
}

pub(crate) fn plan_set_link_properties(
    source: &ByteStore,
    page: PlannerPage<'_>,
    page_index: usize,
    (link, target, look): (Reference, Option<&Target>, Option<Look>),
) -> Result<Plan, SpikeError> {
    let credential = page.credential;
    let rect = link_box((source, credential), page_index, link)?;
    if target.is_none() && look.is_none() {
        return Err(refused("this asks for no change to the link"));
    }
    let mut entries: Vec<(&[u8], String)> = Vec::new();
    if let Some(target) = target {
        let pages = pages_of((source, credential))?;
        entries.extend(entries_of((source, credential), target, &pages)?);
    }
    if let Some(look) = look {
        if !look.width.is_finite() || !(0.0..=100.0).contains(&look.width) {
            return Err(refused(
                "a link's border is between no points and a hundred",
            ));
        }
        entries.extend(look_entries(look));
    }
    let writes = vec![set_entries((source, credential), link, &entries)?];
    if let Some(target) = target {
        prove_link(
            (source, credential),
            &writes,
            (page_index, link),
            (rect, target),
        )?;
    }
    if let Some(look) = look {
        prove_look((source, credential), &writes, (page_index, link), look)?;
    }
    Ok(plan_of(&page, page_index, writes, rect))
}

pub(crate) fn plan_set_link_box(
    source: &ByteStore,
    page: PlannerPage<'_>,
    page_index: usize,
    (link, rect): (Reference, [f64; 4]),
) -> Result<Plan, SpikeError> {
    let credential = page.credential;
    let was = link_box((source, credential), page_index, link)?;
    let rect = checked_box(rect)?;
    let writes = vec![set_entries(
        (source, credential),
        link,
        &[(
            b"/Rect",
            format!("[{} {} {} {}]", rect[0], rect[1], rect[2], rect[3]),
        )],
    )?];
    prove_box((source, credential), &writes, (page_index, link), rect)?;
    let region = [
        was[0].min(rect[0]),
        was[1].min(rect[1]),
        was[2].max(rect[2]),
        was[3].max(rect[3]),
    ];
    Ok(plan_of(&page, page_index, writes, region))
}

pub(crate) fn plan_set_link_boxes(
    source: &ByteStore,
    page: PlannerPage<'_>,
    page_index: usize,
    boxes: &[(Reference, [f64; 4])],
) -> Result<Plan, SpikeError> {
    let credential = page.credential;
    for (at, (link, _)) in boxes.iter().enumerate() {
        if boxes[..at].iter().any(|(earlier, _)| earlier == link) {
            return Err(refused("a link was named twice"));
        }
    }
    let before = links_of(source, credential, page_index)?;
    let writes =
        crate::field_group::in_sequence((source, credential), boxes, |document, (link, rect)| {
            let plan = plan_set_link_box(document, page, page_index, (*link, *rect))?;
            Ok(plan.writes().to_vec())
        })?;
    let region = region_of(
        boxes.iter().map(|(_, rect)| *rect).chain(
            before
                .iter()
                .filter(|(link, _, _)| boxes.iter().any(|(named, _)| named == link))
                .map(|(_, rect, _)| *rect),
        ),
    )
    .ok_or_else(|| refused("no link was named"))?;
    Ok(plan_of(&page, page_index, writes, region))
}

pub(crate) fn plan_remove_links(
    source: &ByteStore,
    page: PlannerPage<'_>,
    page_index: usize,
    links: &[Reference],
) -> Result<Plan, SpikeError> {
    let credential = page.credential;
    let before = links_of(source, credential, page_index)?;
    let region = region_of(
        before
            .iter()
            .filter(|(link, _, _)| links.contains(link))
            .map(|(_, rect, _)| *rect),
    )
    .ok_or_else(|| refused("this is not a link of this page"))?;
    let writes = crate::field_group::in_sequence((source, credential), links, |document, link| {
        let plan = plan_remove_link(document, page, page_index, *link)?;
        Ok(plan.writes().to_vec())
    })?;
    Ok(plan_of(&page, page_index, writes, region))
}

fn region_of(boxes: impl IntoIterator<Item = [f64; 4]>) -> Option<[f64; 4]> {
    boxes.into_iter().reduce(|one, other| {
        [
            one[0].min(other[0]),
            one[1].min(other[1]),
            one[2].max(other[2]),
            one[3].max(other[3]),
        ]
    })
}

pub(crate) fn plan_remove_link(
    source: &ByteStore,
    page: PlannerPage<'_>,
    page_index: usize,
    link: Reference,
) -> Result<Plan, SpikeError> {
    let credential = page.credential;
    let rect = link_box((source, credential), page_index, link)?;
    let writes = vec![crate::new_field::unlisted_on_page(
        (source, credential),
        page.program.page,
        link,
    )?];
    let document = crate::block_rewrite::commit_writes(
        source,
        &writes,
        (credential, crate::Restrictions::SetAside),
    )?;
    if links_of(&document, credential, page_index)?
        .iter()
        .any(|(reference, _, _)| *reference == link)
    {
        return Err(refused("the link is still on the page"));
    }
    Ok(plan_of(&page, page_index, writes, rect))
}

pub type Listed = (Reference, [f64; 4], Option<Target>);

pub fn links_of(
    source: &ByteStore,
    credential: &[u8],
    page_index: usize,
) -> Result<Vec<Listed>, SpikeError> {
    let resolver = pdf_content::open_link_resolver(
        source,
        pdf_content::PageContentLimits::default(),
        credential,
    )
    .map_err(|_| refused("this document's links cannot be read"))?;
    let links = resolver
        .links(page_index)
        .map_err(|_| refused("this page's links cannot be read"))?;
    let listed: Vec<Reference> = links
        .links
        .iter()
        .filter_map(|link| link.reference)
        .collect();
    let named = names_of(source, credential, &listed).unwrap_or_else(|_| vec![None; listed.len()]);
    let mut named = named.into_iter();
    Ok(links
        .links
        .iter()
        .filter_map(|link| {
            let reference = link.reference?;
            let target = named
                .next()
                .flatten()
                .map(Target::Name)
                .or_else(|| target_of(&link.followed()));
            Some((reference, link.rect, target))
        })
        .collect())
}

fn named_here((source, credential): (&ByteStore, &[u8])) -> Vec<String> {
    let Ok(resolver) = pdf_content::open_link_resolver(
        source,
        pdf_content::PageContentLimits::default(),
        credential,
    ) else {
        return Vec::new();
    };
    resolver
        .destinations()
        .into_iter()
        .filter_map(|(name, _)| String::from_utf8(name).ok())
        .collect()
}

pub fn names_of(
    source: &ByteStore,
    credential: &[u8],
    links: &[Reference],
) -> Result<Vec<Option<String>>, SpikeError> {
    let reader = crate::form::Reader::open(source, credential)?;
    let held = named_here((source, credential));
    Ok(links
        .iter()
        .map(|link| {
            let annotation = reader.at(*link)?;
            let destination = reader.entry(&annotation, b"/Dest")?;
            let name = match destination.value.kind() {
                pdf_syntax::ObjectKind::Name => crate::form::Reader::name(&destination)?,
                pdf_syntax::ObjectKind::LiteralString | pdf_syntax::ObjectKind::HexString => {
                    String::from_utf8(reader.bytes(&destination)?).ok()?
                }
                _ => return None,
            };
            held.contains(&name).then_some(name)
        })
        .collect())
}

pub fn look_of(source: &ByteStore, credential: &[u8], link: Reference) -> Result<Look, SpikeError> {
    let reader = crate::form::Reader::open(source, credential)?;
    read_look(&reader, link).ok_or_else(|| refused("this link cannot be read"))
}

fn read_look(reader: &crate::form::Reader, link: Reference) -> Option<Look> {
    let annotation = reader.at(link)?;
    let by_style = reader.entry(&annotation, b"/BS");
    let width = by_style
        .as_ref()
        .and_then(|style| reader.entry(style, b"/W"))
        .and_then(|found| crate::form::Reader::number(&found))
        .or_else(|| {
            let border = reader.entry(&annotation, b"/Border")?;
            let pdf_syntax::ObjectKind::Array(items) = border.value.kind() else {
                return None;
            };
            let third = items.get(2)?;
            crate::form::Reader::number(&reader.follow(&border, third)?)
        })
        .filter(|width| width.is_finite() && (0.0..=100.0).contains(width))
        .unwrap_or(1.0);
    let style = by_style
        .and_then(|style| reader.entry(&style, b"/S"))
        .and_then(|found| crate::form::Reader::name(&found))
        .and_then(|name| LinkBorder::of_name(&name))
        .unwrap_or_default();
    let colour = colour_of(reader, &annotation);
    let highlight = reader
        .entry(&annotation, b"/H")
        .and_then(|found| crate::form::Reader::name(&found))
        .and_then(|name| Highlight::of_name(&name))
        .unwrap_or_default();
    Some(Look {
        width,
        style,
        colour,
        highlight,
    })
}

pub fn looks_of(
    source: &ByteStore,
    credential: &[u8],
    links: &[Reference],
) -> Result<Vec<Look>, SpikeError> {
    let reader = crate::form::Reader::open(source, credential)?;
    Ok(links
        .iter()
        .map(|link| read_look(&reader, *link).unwrap_or_default())
        .collect())
}

fn colour_of(reader: &crate::form::Reader, annotation: &crate::form::Found) -> Option<[f64; 3]> {
    let colour = reader.entry(annotation, b"/C")?;
    let pdf_syntax::ObjectKind::Array(items) = colour.value.kind() else {
        return None;
    };
    let parts: Vec<f64> = items
        .iter()
        .map(|item| {
            reader
                .follow(&colour, item)
                .and_then(|found| crate::form::Reader::number(&found))
        })
        .collect::<Option<Vec<f64>>>()?;
    if !parts.iter().all(|part| (0.0..=1.0).contains(part)) {
        return None;
    }
    match parts.as_slice() {
        [grey] => Some([*grey; 3]),
        [red, green, blue] => Some([*red, *green, *blue]),
        [cyan, magenta, yellow, black] => Some([
            (1.0 - cyan) * (1.0 - black),
            (1.0 - magenta) * (1.0 - black),
            (1.0 - yellow) * (1.0 - black),
        ]),
        _ => None,
    }
}

fn look_entries(look: Look) -> Vec<(&'static [u8], String)> {
    let width = trimmed(look.width);
    let dashes = if look.style == LinkBorder::Dashed {
        " /D [3]"
    } else {
        ""
    };
    let colour = match look.colour {
        Some([red, green, blue]) => format!(
            "[{} {} {}]",
            trimmed(red.clamp(0.0, 1.0)),
            trimmed(green.clamp(0.0, 1.0)),
            trimmed(blue.clamp(0.0, 1.0))
        ),
        None => String::new(),
    };
    vec![
        (
            b"/BS".as_slice(),
            format!(
                "<< /Type /Border /W {width} /S /{}{dashes} >>",
                look.style.name()
            ),
        ),
        (b"/Border", format!("[0 0 {width}]")),
        (b"/H", format!("/{}", look.highlight.name())),
        (b"/C", colour),
    ]
}

fn trimmed(number: f64) -> String {
    let text = format!("{number:.4}");
    let text = text.trim_end_matches('0').trim_end_matches('.');
    if text.is_empty() {
        "0".to_owned()
    } else {
        text.to_owned()
    }
}

fn prove_look(
    (source, credential): (&ByteStore, &[u8]),
    writes: &[PlannedWrite],
    (page_index, link): (usize, Reference),
    look: Look,
) -> Result<(), SpikeError> {
    let document = crate::block_rewrite::commit_writes(
        source,
        writes,
        (credential, crate::Restrictions::SetAside),
    )?;
    if !links_of(&document, credential, page_index)?
        .iter()
        .any(|(reference, _, _)| *reference == link)
    {
        return Err(refused("the link does not read back on the page"));
    }
    let read = look_of(&document, credential, link)?;
    let close = |one: f64, other: f64| (one - other).abs() < 1e-4;
    if !close(read.width, look.width)
        || read.style != look.style
        || read.highlight != look.highlight
    {
        return Err(refused("the link does not read back drawn as it was asked"));
    }
    match (read.colour, look.colour) {
        (None, None) => Ok(()),
        (Some(read), Some(asked))
            if read
                .iter()
                .zip(asked)
                .all(|(one, other)| close(*one, other)) =>
        {
            Ok(())
        }
        _ => Err(refused("the link's colour does not read back")),
    }
}

#[must_use]
pub fn target_of(followed: &pdf_content::Followed<'_>) -> Option<Target> {
    match followed {
        pdf_content::Followed::Uri(uri) => {
            Some(Target::Address(String::from_utf8_lossy(uri).into_owned()))
        }
        pdf_content::Followed::Page(destination) => Some(Target::Page(
            destination.page?,
            arrival_of(destination.view),
        )),
        pdf_content::Followed::Other(pdf_content::LinkAction::RemoteGoTo { file, destination }) => {
            let file = file.as_ref()?;
            let destination = (*destination)?;
            Some(Target::Document {
                file: String::from_utf8_lossy(file).into_owned(),
                page: destination.page?,
                arrival: arrival_of(destination.view),
            })
        }
        pdf_content::Followed::Other(_) | pdf_content::Followed::Nothing => None,
    }
}

#[must_use]
pub fn arrival_of(view: pdf_content::View) -> Arrival {
    match view {
        pdf_content::View::Fit => Arrival::FitPage,
        pdf_content::View::FitH { .. } | pdf_content::View::FitBH { .. } => Arrival::FitWidth,
        pdf_content::View::FitV { .. } | pdf_content::View::FitBV { .. } => Arrival::FitHeight,
        pdf_content::View::FitB | pdf_content::View::FitR { .. } => Arrival::FitVisible,
        pdf_content::View::Xyz { zoom, .. } => match zoom {
            Some(zoom) if zoom > 0.0 => {
                if (zoom - 1.0).abs() < 1e-3 {
                    Arrival::ActualSize
                } else {
                    Arrival::Percent(zoom * 100.0)
                }
            }
            _ => Arrival::InheritZoom,
        },
        pdf_content::View::Unknown => Arrival::InheritZoom,
    }
}

fn link_box(
    (source, credential): (&ByteStore, &[u8]),
    page_index: usize,
    link: Reference,
) -> Result<[f64; 4], SpikeError> {
    links_of(source, credential, page_index)?
        .into_iter()
        .find(|(reference, _, _)| *reference == link)
        .map(|(_, rect, _)| rect)
        .ok_or_else(|| refused("this is not a link of this page"))
}

fn prove_link(
    (source, credential): (&ByteStore, &[u8]),
    writes: &[PlannedWrite],
    (page_index, link): (usize, Reference),
    (rect, target): ([f64; 4], &Target),
) -> Result<(), SpikeError> {
    let document = crate::block_rewrite::commit_writes(
        source,
        writes,
        (credential, crate::Restrictions::SetAside),
    )?;
    let found = links_of(&document, credential, page_index)?
        .into_iter()
        .find(|(reference, _, _)| *reference == link)
        .ok_or_else(|| refused("the link does not read back on the page"))?;
    let close = |one: f64, other: f64| (one - other).abs() < 1e-6;
    if !found
        .1
        .iter()
        .zip(rect)
        .all(|(one, other)| close(*one, other))
    {
        return Err(refused("the link does not read back in the box asked for"));
    }
    if found.2.as_ref() != Some(target) {
        return Err(refused(
            "the link does not read back going where it was sent",
        ));
    }
    Ok(())
}

fn prove_box(
    (source, credential): (&ByteStore, &[u8]),
    writes: &[PlannedWrite],
    (page_index, link): (usize, Reference),
    rect: [f64; 4],
) -> Result<(), SpikeError> {
    let document = crate::block_rewrite::commit_writes(
        source,
        writes,
        (credential, crate::Restrictions::SetAside),
    )?;
    let found = links_of(&document, credential, page_index)?
        .into_iter()
        .find(|(reference, _, _)| *reference == link)
        .ok_or_else(|| refused("the link does not read back on the page"))?;
    let close = |one: f64, other: f64| (one - other).abs() < 1e-6;
    if found
        .1
        .iter()
        .zip(rect)
        .all(|(one, other)| close(*one, other))
    {
        Ok(())
    } else {
        Err(refused("the link does not read back in the box asked for"))
    }
}

#[cfg(test)]
#[expect(
    clippy::float_cmp,
    reason = "a box written as numbers is read back as the same numbers"
)]
mod tests {
    use pdf_bytes::ByteStore;
    use pdf_syntax::Reference;

    use super::{Arrival, Target};
    use crate::plan::Command;
    use crate::spike_move_text::{SpikeError, plan_command_with_fonts};

    fn two_pages() -> ByteStore {
        crate::new_field::tests::document(&[
            "<< /Type /Catalog /Pages 2 0 R >>",
            "<< /Type /Pages /MediaBox [0 0 300 300] /Kids [3 0 R 4 0 R] /Count 2 >>",
            "<< /Type /Page /Parent 2 0 R /Contents 5 0 R /Resources << >> >>",
            "<< /Type /Page /Parent 2 0 R /Contents 5 0 R /Resources << >> >>",
            "<< /Length 21 >>\nstream\n0 0 m 100 100 l S    \nendstream",
        ])
    }

    fn after(source: &ByteStore, command: &Command) -> Result<ByteStore, SpikeError> {
        let plan = plan_command_with_fonts(source, command, b"", None)?;
        crate::block_rewrite::commit_writes(
            source,
            plan.writes(),
            (b"", crate::Restrictions::Respect),
        )
    }

    fn links(source: &ByteStore) -> Vec<super::Listed> {
        super::links_of(source, b"", 0).expect("the page's links read")
    }

    fn added(rect: [f64; 4], target: Target) -> Command {
        Command::AddLink {
            page_index: 0,
            rect,
            target,
            look: super::Look::default(),
        }
    }

    #[test]
    fn a_link_to_an_address_reads_back() {
        let source = after(
            &two_pages(),
            &added(
                [20.0, 200.0, 180.0, 220.0],
                Target::Address("https://example.org/a b".to_owned()),
            ),
        )
        .expect("the link is added");
        let found = links(&source);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].1, [20.0, 200.0, 180.0, 220.0]);
        assert_eq!(
            found[0].2,
            Some(Target::Address("https://example.org/a b".to_owned()))
        );
    }

    #[test]
    fn a_link_to_a_page_reads_back() {
        let source = after(
            &two_pages(),
            &added(
                [20.0, 200.0, 180.0, 220.0],
                Target::Page(1, Arrival::InheritZoom),
            ),
        )
        .expect("the link is added");
        assert_eq!(
            links(&source)[0].2,
            Some(Target::Page(1, Arrival::InheritZoom))
        );
    }

    #[test]
    fn a_link_sent_to_a_page_no_longer_opens_its_address() {
        let source = after(
            &two_pages(),
            &added(
                [20.0, 200.0, 180.0, 220.0],
                Target::Address("https://example.org/".to_owned()),
            ),
        )
        .expect("the link is added");
        let link = links(&source)[0].0;
        let sent = after(
            &source,
            &Command::SetLinkProperties {
                page_index: 0,
                link,
                target: Some(Target::Page(1, Arrival::InheritZoom)),
                look: None,
            },
        )
        .expect("the link is sent to the page");
        let found = links(&sent);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].0, link);
        assert_eq!(found[0].2, Some(Target::Page(1, Arrival::InheritZoom)));
        let body = crate::object_edit::ObjectEdit::of(&sent, link, b"")
            .expect("the link object reads")
            .body
            .bytes;
        assert!(!body.windows(4).any(|window| window == b"/URI"));
    }

    #[test]
    fn a_links_box_moves() {
        let source = after(
            &two_pages(),
            &added(
                [20.0, 200.0, 180.0, 220.0],
                Target::Page(1, Arrival::InheritZoom),
            ),
        )
        .expect("the link is added");
        let link = links(&source)[0].0;
        let moved = after(
            &source,
            &Command::SetLinkBox {
                page_index: 0,
                link,
                rect: [90.0, 60.0, 30.0, 40.0],
            },
        )
        .expect("the link's box is set");
        assert_eq!(links(&moved)[0].1, [30.0, 40.0, 90.0, 60.0]);
        assert_eq!(
            links(&moved)[0].2,
            Some(Target::Page(1, Arrival::InheritZoom))
        );
    }

    #[test]
    fn a_link_removed_is_gone() {
        let source = after(
            &two_pages(),
            &added(
                [20.0, 200.0, 180.0, 220.0],
                Target::Page(1, Arrival::InheritZoom),
            ),
        )
        .expect("the link is added");
        let link = links(&source)[0].0;
        let taken = after(
            &source,
            &Command::RemoveLink {
                page_index: 0,
                link,
            },
        )
        .expect("the link is removed");
        assert!(links(&taken).is_empty());
    }

    #[test]
    fn what_is_refused() {
        let source = two_pages();
        for command in [
            added(
                [20.0, 200.0, 21.0, 220.0],
                Target::Page(0, Arrival::InheritZoom),
            ),
            added(
                [20.0, 200.0, 180.0, f64::NAN],
                Target::Page(0, Arrival::InheritZoom),
            ),
            added(
                [20.0, 200.0, 180.0, 220.0],
                Target::Page(7, Arrival::InheritZoom),
            ),
            added(
                [20.0, 200.0, 180.0, 220.0],
                Target::Address("  ".to_owned()),
            ),
            added(
                [20.0, 200.0, 180.0, 220.0],
                Target::Address("http://a\nb".to_owned()),
            ),
        ] {
            assert!(
                after(&source, &command).is_err(),
                "this should be refused: {command:?}"
            );
        }
    }

    #[test]
    fn a_new_link_is_invisible() {
        let source = after(
            &two_pages(),
            &added(
                [20.0, 200.0, 180.0, 220.0],
                Target::Page(1, Arrival::InheritZoom),
            ),
        )
        .expect("the link is added");
        let link = links(&source)[0].0;
        let look = super::look_of(&source, b"", link).expect("the look reads");
        assert_eq!(look, super::Look::default());
        assert_eq!(look.width, 0.0);
    }

    #[test]
    fn a_visible_rectangle_reads_back_both_ways() {
        let source = after(
            &two_pages(),
            &added(
                [20.0, 200.0, 180.0, 220.0],
                Target::Page(1, Arrival::InheritZoom),
            ),
        )
        .expect("the link is added");
        let link = links(&source)[0].0;
        let look = super::Look {
            width: 2.0,
            style: super::LinkBorder::Dashed,
            colour: Some([1.0, 0.0, 0.0]),
            highlight: super::Highlight::Outline,
        };
        let drawn = after(
            &source,
            &Command::SetLinkProperties {
                page_index: 0,
                link,
                target: None,
                look: Some(look),
            },
        )
        .expect("the look is set");
        assert_eq!(super::look_of(&drawn, b"", link).expect("reads"), look);
        let body = crate::object_edit::ObjectEdit::of(&drawn, link, b"")
            .expect("the link object reads")
            .body
            .bytes;
        let written = String::from_utf8_lossy(&body).into_owned();
        assert!(written.contains("/Border [0 0 2]"), "{written}");
        assert!(written.contains("/S /D"), "{written}");
        assert!(
            written.contains("/D [3]"),
            "a dashed border says how long its dashes are: {written}"
        );
        assert!(written.contains("/H /O"), "{written}");
        assert!(written.contains("/C [1 0 0]"), "{written}");
    }

    #[test]
    fn a_link_made_invisible_again_has_no_border_left() {
        let source = after(
            &two_pages(),
            &added(
                [20.0, 200.0, 180.0, 220.0],
                Target::Page(1, Arrival::InheritZoom),
            ),
        )
        .expect("the link is added");
        let link = links(&source)[0].0;
        let drawn = after(
            &source,
            &Command::SetLinkProperties {
                page_index: 0,
                link,
                target: None,
                look: Some(super::Look {
                    width: 3.0,
                    style: super::LinkBorder::Underline,
                    colour: Some([0.0, 0.0, 1.0]),
                    highlight: super::Highlight::None,
                }),
            },
        )
        .expect("the look is set");
        let hidden = after(
            &drawn,
            &Command::SetLinkProperties {
                page_index: 0,
                link,
                target: None,
                look: Some(super::Look::default()),
            },
        )
        .expect("the look is set again");
        let look = super::look_of(&hidden, b"", link).expect("the look reads");
        assert_eq!(look, super::Look::default());
        let body = crate::object_edit::ObjectEdit::of(&hidden, link, b"")
            .expect("the link object reads")
            .body
            .bytes;
        let written = String::from_utf8_lossy(&body).into_owned();
        assert!(written.contains("/Border [0 0 0]"), "{written}");
        assert!(!written.contains("/C"), "{written}");
    }

    #[test]
    fn a_border_style_wins_over_the_older_border_array() {
        let source = crate::new_field::tests::document(&[
            "<< /Type /Catalog /Pages 2 0 R >>",
            "<< /Type /Pages /MediaBox [0 0 300 300] /Kids [3 0 R] /Count 1 >>",
            "<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << >> \
              /Annots [5 0 R] >>",
            "<< /Length 21 >>\nstream\n0 0 m 100 100 l S    \nendstream",
            "<< /Type /Annot /Subtype /Link /Rect [20 200 180 220] /Border [0 0 5] \
              /BS << /W 2 /S /U >> /C [0 0.5 0] /H /P /Dest [3 0 R /Fit] >>",
        ]);
        let look = super::look_of(&source, b"", Reference::new(5, 0)).expect("the look reads");
        assert_eq!(look.width, 2.0, "/BS says two points, /Border says five");
        assert_eq!(look.style, super::LinkBorder::Underline);
        assert_eq!(look.highlight, super::Highlight::Inset);
        assert_eq!(look.colour, Some([0.0, 0.5, 0.0]));
    }

    #[test]
    fn a_look_that_is_not_one_is_refused() {
        let source = after(
            &two_pages(),
            &added(
                [20.0, 200.0, 180.0, 220.0],
                Target::Page(1, Arrival::InheritZoom),
            ),
        )
        .expect("the link is added");
        let link = links(&source)[0].0;
        for (page_index, width) in [(0, f64::NAN), (0, -1.0), (0, 1000.0), (1, 2.0)] {
            let command = Command::SetLinkProperties {
                page_index,
                link,
                target: None,
                look: Some(super::Look {
                    width,
                    ..super::Look::default()
                }),
            };
            let why = after(&source, &command).expect_err("this should be refused");
            let said = format!("{why:?}");
            assert!(
                said.contains("a hundred") || said.contains("not a link of this page"),
                "refused for the wrong reason: {said}"
            );
        }
    }

    #[test]
    fn every_arrival_reads_back_as_itself() {
        for (arrival, written) in [
            (Arrival::InheritZoom, "/XYZ null null null"),
            (Arrival::FitPage, "/Fit"),
            (Arrival::FitWidth, "/FitH 300"),
            (Arrival::FitHeight, "/FitV 0"),
            (Arrival::FitVisible, "/FitB"),
            (Arrival::ActualSize, "/XYZ 0 300 1"),
            (Arrival::Percent(150.0), "/XYZ 0 300 1.5"),
        ] {
            let source = after(
                &two_pages(),
                &added([20.0, 200.0, 180.0, 220.0], Target::Page(1, arrival)),
            )
            .expect("the link is added");
            let found = links(&source);
            assert_eq!(found[0].2, Some(Target::Page(1, arrival)), "read back");
            let body = crate::object_edit::ObjectEdit::of(&source, found[0].0, b"")
                .expect("the link object reads")
                .body
                .bytes;
            let text = String::from_utf8_lossy(&body).into_owned();
            assert!(
                text.contains(written),
                "{arrival:?} is written {written}: {text}"
            );
        }
    }

    #[test]
    fn a_zoom_that_is_not_one_is_refused() {
        for percent in [0.0, -100.0, f64::NAN, 100_000.0] {
            let command = added(
                [20.0, 200.0, 180.0, 220.0],
                Target::Page(1, Arrival::Percent(percent)),
            );
            assert!(
                after(&two_pages(), &command).is_err(),
                "this zoom should be refused: {percent}"
            );
        }
    }

    #[test]
    fn a_destination_of_another_shape_is_read_as_the_nearest_offered() {
        let source = crate::new_field::tests::document(&[
            "<< /Type /Catalog /Pages 2 0 R >>",
            "<< /Type /Pages /MediaBox [0 0 300 300] /Kids [3 0 R] /Count 1 >>",
            "<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << >> \
              /Annots [5 0 R 6 0 R 7 0 R] >>",
            "<< /Length 21 >>\nstream\n0 0 m 100 100 l S    \nendstream",
            "<< /Type /Annot /Subtype /Link /Rect [20 200 180 220] \
              /Dest [3 0 R /FitR 0 0 100 100] >>",
            "<< /Type /Annot /Subtype /Link /Rect [20 100 180 120] \
              /Dest [3 0 R /XYZ 10 20 0] >>",
            "<< /Type /Annot /Subtype /Link /Rect [20 20 180 40] \
              /Dest [3 0 R /FitBH 200] >>",
        ]);
        let found = links(&source);
        assert_eq!(
            found[0].2,
            Some(Target::Page(0, Arrival::FitVisible)),
            "/FitR"
        );
        assert_eq!(
            found[1].2,
            Some(Target::Page(0, Arrival::InheritZoom)),
            "a zoom of zero keeps what the reader has"
        );
        assert_eq!(
            found[2].2,
            Some(Target::Page(0, Arrival::FitWidth)),
            "/FitBH"
        );
    }

    #[test]
    fn several_links_move_at_once() {
        let mut source = two_pages();
        for at in 0..3 {
            let top = 200.0 - f64::from(at) * 40.0;
            source = after(
                &source,
                &added(
                    [20.0, top, 180.0, top + 20.0],
                    Target::Page(1, Arrival::FitPage),
                ),
            )
            .expect("the link is added");
        }
        let found = links(&source);
        assert_eq!(found.len(), 3);
        let boxes: Vec<(Reference, [f64; 4])> = found
            .iter()
            .map(|(link, [_, y0, _, y1], _)| (*link, [50.0, *y0, 150.0, *y1]))
            .collect();
        let moved = after(
            &source,
            &Command::SetLinkBoxes {
                page_index: 0,
                boxes: boxes.clone(),
            },
        )
        .expect("the links move");
        let after_move = links(&moved);
        assert_eq!(after_move.len(), 3);
        for (link, rect) in &boxes {
            let found = after_move
                .iter()
                .find(|(reference, _, _)| reference == link)
                .expect("the link is still there");
            assert_eq!(found.1, *rect, "aligned to the box asked for");
            assert_eq!(
                found.2,
                Some(Target::Page(1, Arrival::FitPage)),
                "still goes there"
            );
        }
    }

    #[test]
    fn several_links_are_taken_off_at_once() {
        let mut source = two_pages();
        for at in 0..3 {
            let top = 200.0 - f64::from(at) * 40.0;
            source = after(
                &source,
                &added(
                    [20.0, top, 180.0, top + 20.0],
                    Target::Page(1, Arrival::InheritZoom),
                ),
            )
            .expect("the link is added");
        }
        let found = links(&source);
        let taken = after(
            &source,
            &Command::RemoveLinks {
                page_index: 0,
                links: vec![found[0].0, found[2].0],
            },
        )
        .expect("the links are removed");
        let left = links(&taken);
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].0, found[1].0, "the middle one is the one left");
    }

    #[test]
    fn a_link_named_twice_is_refused() {
        let source = after(
            &two_pages(),
            &added(
                [20.0, 200.0, 180.0, 220.0],
                Target::Page(1, Arrival::InheritZoom),
            ),
        )
        .expect("the link is added");
        let link = links(&source)[0].0;
        let command = Command::SetLinkBoxes {
            page_index: 0,
            boxes: vec![
                (link, [10.0, 10.0, 90.0, 40.0]),
                (link, [20.0, 20.0, 99.0, 50.0]),
            ],
        };
        assert!(after(&source, &command).is_err());
    }

    #[test]
    fn a_link_to_another_document_reads_back() {
        let target = Target::Document {
            file: "handbook.pdf".to_owned(),
            page: 11,
            arrival: Arrival::FitPage,
        };
        let source = after(
            &two_pages(),
            &added([20.0, 200.0, 180.0, 220.0], target.clone()),
        )
        .expect("the link is added");
        let found = links(&source);
        assert_eq!(found[0].2, Some(target));
        let body = crate::object_edit::ObjectEdit::of(&source, found[0].0, b"")
            .expect("the link object reads")
            .body
            .bytes;
        let written = String::from_utf8_lossy(&body).into_owned();
        assert!(written.contains("/S /GoToR"), "{written}");
        assert!(written.contains("(handbook.pdf)"), "{written}");
        assert!(
            written.contains("/D [11 /Fit]"),
            "the page is counted in that document: {written}"
        );
    }

    #[test]
    fn a_document_link_that_is_not_one_is_refused() {
        for target in [
            Target::Document {
                file: "   ".to_owned(),
                page: 0,
                arrival: Arrival::FitPage,
            },
            Target::Document {
                file: "a\nb.pdf".to_owned(),
                page: 0,
                arrival: Arrival::FitPage,
            },
            Target::Document {
                file: "b.pdf".to_owned(),
                page: 0,
                arrival: Arrival::Percent(0.0),
            },
        ] {
            let command = added([20.0, 200.0, 180.0, 220.0], target.clone());
            assert!(
                after(&two_pages(), &command).is_err(),
                "this should be refused: {target:?}"
            );
        }
    }

    #[test]
    fn a_link_of_another_kind_is_listed_without_a_target() {
        let source = crate::new_field::tests::document(&[
            "<< /Type /Catalog /Pages 2 0 R >>",
            "<< /Type /Pages /MediaBox [0 0 300 300] /Kids [3 0 R] /Count 1 >>",
            "<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << >> \
              /Annots [5 0 R] >>",
            "<< /Length 21 >>\nstream\n0 0 m 100 100 l S    \nendstream",
            "<< /Type /Annot /Subtype /Link /Rect [20 200 180 220] \
              /A << /S /Launch /F (a.exe) >> >>",
        ]);
        let found = links(&source);
        assert_eq!(found.len(), 1, "it is on the page");
        assert_eq!(found[0].2, None, "and this window writes no such target");
        let moved = after(
            &source,
            &Command::SetLinkBox {
                page_index: 0,
                link: found[0].0,
                rect: [10.0, 10.0, 110.0, 40.0],
            },
        )
        .expect("a link of any kind moves");
        assert_eq!(links(&moved)[0].1, [10.0, 10.0, 110.0, 40.0]);
    }

    #[test]
    fn a_link_of_another_page_is_not_this_pages() {
        let source = after(
            &two_pages(),
            &added(
                [20.0, 200.0, 180.0, 220.0],
                Target::Page(1, Arrival::InheritZoom),
            ),
        )
        .expect("the link is added");
        let link = links(&source)[0].0;
        for command in [
            Command::RemoveLink {
                page_index: 1,
                link,
            },
            Command::SetLinkBox {
                page_index: 1,
                link,
                rect: [10.0, 10.0, 90.0, 40.0],
            },
            Command::SetLinkProperties {
                page_index: 1,
                link,
                target: Some(Target::Page(0, Arrival::InheritZoom)),
                look: None,
            },
        ] {
            assert!(
                after(&source, &command).is_err(),
                "the second page has no such link: {command:?}"
            );
        }
    }
}
