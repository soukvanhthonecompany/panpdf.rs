use std::sync::Arc;

use pdf_bytes::{ByteStore, SourceId};
use pdf_security::{Allowed, CipherMethod, PrintAllowance};
use pdf_syntax::Reference;

use crate::form::FieldValue;
use crate::history::History;
use crate::info::Lock as Opens;
use crate::plan::{Command, PenStep, PenStroke};
use crate::reprotect::{Asked, Wanted};
use crate::spike_move_text::{SpikeError, plan_command_under, read_page};
use crate::{Password, Restrictions};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Locked {
    Plain,
    OwnerOnly,
    UserAes,
    UserRc4,
    Forbidding,
}

const EVERY_LOCK: [Locked; 5] = [
    Locked::Plain,
    Locked::OwnerOnly,
    Locked::UserAes,
    Locked::UserRc4,
    Locked::Forbidding,
];

impl Locked {
    fn credential(self) -> &'static [u8] {
        match self {
            Self::Plain | Self::OwnerOnly => b"",
            Self::UserAes | Self::Forbidding => b"panpdf",
            Self::UserRc4 => b"view",
        }
    }

    fn cipher(self) -> Option<CipherMethod> {
        match self {
            Self::Plain => None,
            Self::UserRc4 => Some(CipherMethod::Rc4),
            Self::OwnerOnly | Self::UserAes | Self::Forbidding => Some(CipherMethod::Aes256),
        }
    }

    fn asks(self) -> bool {
        !matches!(self, Self::Plain | Self::OwnerOnly)
    }

    fn restrictions(self) -> Restrictions {
        if self == Self::Forbidding {
            Restrictions::SetAside
        } else {
            Restrictions::Respect
        }
    }

    fn lock(self, plain: &ByteStore) -> ByteStore {
        let aes = |user: &[u8], allowed: Allowed| {
            let written = crate::reprotect::rewrite(
                plain,
                b"",
                &Wanted::Protected(Box::new(Asked {
                    user: user.to_vec(),
                    owner: b"owner-2026".to_vec(),
                    allowed,
                })),
            )
            .expect("the document is protected");
            ByteStore::new(SourceId::next_document(), Arc::<[u8]>::from(written))
        };
        match self {
            Self::Plain => plain.clone(),
            Self::OwnerOnly => aes(b"", Allowed::default()),
            Self::UserAes => aes(b"panpdf", Allowed::default()),
            Self::UserRc4 => crate::reprotect::locked_rc4(plain),
            Self::Forbidding => aes(
                b"panpdf",
                Allowed {
                    print: PrintAllowance::Refused,
                    modify: false,
                    copy: false,
                    annotate: false,
                    fill_forms: false,
                    assemble: false,
                },
            ),
        }
    }
}

fn edited(locked: &ByteStore, lock: Locked, command: &Command) -> Result<ByteStore, SpikeError> {
    let credential = lock.credential();
    let fonts = crate::new_text::tests::provider();
    let mut history = History::new(locked.clone(), credential);
    if lock.restrictions() == Restrictions::SetAside {
        history.set_aside_restrictions();
    }
    let plan = plan_command_under(
        history.source(),
        command,
        credential,
        (Some(&fonts), lock.restrictions()),
    )?;
    history.apply(plan)?;
    let after = history.source().clone();
    let facts = crate::info::document_facts(&after, credential).expect("it describes itself");
    assert_eq!(
        facts.protection.map(|protection| protection.stream_cipher),
        lock.cipher(),
        "{lock:?}: the protection is the one it had"
    );
    if lock.asks() {
        assert_eq!(
            crate::info::lock(&after, b""),
            Opens::Refused,
            "{lock:?}: it still asks for its password"
        );
    }
    Ok(after)
}

fn under_every_lock(
    plain: &ByteStore,
    command: impl Fn(&ByteStore, Locked) -> Command,
    check: impl Fn(&ByteStore, &[u8], Locked),
) {
    for lock in EVERY_LOCK {
        let locked = lock.lock(plain);
        let after = edited(&locked, lock, &command(&locked, lock))
            .unwrap_or_else(|error| panic!("{lock:?}: the edit is made: {error}"));
        check(&after, lock.credential(), lock);
    }
}

fn a_document() -> ByteStore {
    crate::new_field::tests::document(&[
        "<< /Type /Catalog /Pages 2 0 R /Outlines 7 0 R /AcroForm << /Fields [4 0 R 5 0 R] \
         /DA (/Helv 0 Tf 0 g) >> >>",
        "<< /Type /Pages /MediaBox [0 0 300 300] /Kids [3 0 R 11 0 R] /Count 2 >>",
        "<< /Type /Page /Parent 2 0 R /Contents 9 0 R /Resources << /ProcSet [/PDF] >> \
         /Annots [4 0 R 5 0 R 6 0 R] >>",
        "<< /Type /Annot /Subtype /Widget /FT /Tx /T (Name) /Rect [20 200 120 220] \
         /DA (/Helv 10 Tf 0 g) /P 3 0 R >>",
        "<< /Type /Annot /Subtype /Widget /FT /Btn /T (Agree) /V /Off /AS /Off \
         /Rect [20 160 36 176] /AP << /N << /Yes 10 0 R /Off 10 0 R >> >> /P 3 0 R >>",
        "<< /Type /Annot /Subtype /Link /Rect [150 20 250 40] /Border [0 0 0] \
         /A << /S /URI /URI (https://example.org/) >> >>",
        "<< /Type /Outlines /First 8 0 R /Last 8 0 R /Count 1 >>",
        "<< /Title (Start) /Parent 7 0 R /Dest [3 0 R /XYZ null null null] >>",
        "<< /Length 25 >>\nstream\n0 0 0 rg 10 10 20 20 re f\nendstream",
        "<< /Type /XObject /Subtype /Form /BBox [0 0 16 16] /Length 0 >>\nstream\n\nendstream",
        "<< /Type /Page /Parent 2 0 R /Contents 12 0 R /Resources << >> >>",
        "<< /Length 25 >>\nstream\n0 0 1 rg 50 50 40 40 re f\nendstream",
    ])
}

fn widget(number: u32) -> Reference {
    Reference::new(number, 0)
}

fn fields(after: &ByteStore, credential: &[u8]) -> Vec<crate::form::FormField> {
    crate::form::fields_of_document(after, credential)
        .expect("the form reads")
        .into_iter()
        .map(|(_, field)| field)
        .collect()
}

fn page_count(after: &ByteStore, credential: &[u8]) -> usize {
    pdf_content::count_pages_with_password(
        after,
        pdf_content::PageContentLimits::default(),
        credential,
    )
    .expect("the pages count")
}

fn atoms(after: &ByteStore, credential: &[u8], page: usize) -> usize {
    read_page(
        after,
        page,
        credential,
        Some(&crate::new_text::tests::provider()),
    )
    .expect("the page reads")
    .graph
    .atoms
    .len()
}

#[test]
fn a_field_is_filled_in() {
    under_every_lock(
        &a_document(),
        |_, _| Command::FillField {
            page_index: 0,
            widget: widget(4),
            value: FieldValue::Text("AB".to_owned()),
        },
        |after, credential, lock| {
            let name = fields(after, credential)
                .into_iter()
                .find(|field| field.name == "Name")
                .expect("the field is there");
            assert_eq!(
                name.value,
                FieldValue::Text("AB".to_owned()),
                "{lock:?}: the value reads back"
            );
        },
    );
    under_every_lock(
        &a_document(),
        |_, _| Command::FillField {
            page_index: 0,
            widget: widget(5),
            value: FieldValue::State("Yes".to_owned()),
        },
        |after, credential, lock| {
            let agree = fields(after, credential)
                .into_iter()
                .find(|field| field.name == "Agree")
                .expect("the box is there");
            assert_eq!(
                agree.value,
                FieldValue::State("Yes".to_owned()),
                "{lock:?}: the box is ticked"
            );
        },
    );
}

#[test]
fn a_field_is_added_renamed_and_ordered() {
    under_every_lock(
        &a_document(),
        |_, _| Command::AddField {
            page_index: 0,
            rect: [150.0, 200.0, 250.0, 220.0],
            kind: crate::new_field::NewFieldKind::Text,
            name: Some("City".to_owned()),
            options: Vec::new(),
        },
        |after, credential, lock| {
            let names: Vec<String> = fields(after, credential)
                .into_iter()
                .map(|field| field.name)
                .collect();
            assert!(names.contains(&"City".to_owned()), "{lock:?}: {names:?}");
            assert!(names.contains(&"Name".to_owned()), "{lock:?}: {names:?}");
        },
    );
    under_every_lock(
        &a_document(),
        |_, _| Command::SetFieldSettings {
            page_index: 0,
            widget: widget(4),
            settings: crate::field_settings::FieldSettings {
                name: Some("Given name".to_owned()),
                ..crate::field_settings::FieldSettings::default()
            },
        },
        |after, credential, lock| {
            assert!(
                fields(after, credential)
                    .iter()
                    .any(|field| field.name == "Given name"),
                "{lock:?}: the field is renamed"
            );
        },
    );
    under_every_lock(
        &a_document(),
        |_, _| Command::SetTabOrder {
            page_index: 0,
            widgets: vec![widget(5), widget(4)],
            order: crate::tab_order::TabOrder::AsListed,
        },
        |after, credential, lock| {
            let order: Vec<Reference> = crate::form::fields_of_page(after, widget(3), credential)
                .expect("the page's fields read")
                .into_iter()
                .map(|field| field.widget)
                .collect();
            assert_eq!(order, vec![widget(5), widget(4)], "{lock:?}");
        },
    );
}

#[test]
fn a_field_is_moved_copied_and_taken_out() {
    under_every_lock(
        &a_document(),
        |_, _| Command::SetFieldBoxes {
            page_index: 0,
            boxes: vec![(widget(4), [20.0, 230.0, 120.0, 250.0])],
        },
        |after, credential, lock| {
            let name = fields(after, credential)
                .into_iter()
                .find(|field| field.name == "Name")
                .expect("the field is there");
            let wanted = [20.0, 230.0, 120.0, 250.0];
            assert!(
                name.rect
                    .iter()
                    .zip(wanted)
                    .all(|(had, want)| (had - want).abs() < 1e-6),
                "{lock:?}: {:?}",
                name.rect
            );
        },
    );
    under_every_lock(
        &a_document(),
        |_, _| Command::CopyFields {
            page_index: 0,
            copies: vec![
                (widget(4), [20.0, 100.0, 120.0, 120.0]),
                (widget(4), [150.0, 100.0, 250.0, 120.0]),
            ],
        },
        |after, credential, lock| {
            let texts: Vec<crate::form::FormField> = fields(after, credential)
                .into_iter()
                .filter(|field| field.kind == crate::form::FieldKind::Text)
                .collect();
            assert_eq!(texts.len(), 3, "{lock:?}: each copy is a field of its own");
            for copy in &texts[1..] {
                assert!(
                    copy.name.starts_with("Name") && copy.name != "Name",
                    "{lock:?}: {:?}",
                    copy.name
                );
                assert_eq!(copy.appearance, texts[0].appearance, "{lock:?}");
            }
        },
    );
    under_every_lock(
        &a_document(),
        |_, _| Command::RemoveField {
            page_index: 0,
            widget: widget(5),
        },
        |after, credential, lock| {
            let names: Vec<String> = fields(after, credential)
                .into_iter()
                .map(|field| field.name)
                .collect();
            assert_eq!(names, vec!["Name".to_owned()], "{lock:?}");
        },
    );
}

#[test]
fn a_link_is_added_changed_and_taken_out() {
    let address = crate::link::Target::Address("https://panpdf.org/ພາສາລາວ".to_owned());
    under_every_lock(
        &a_document(),
        |_, _| Command::AddLink {
            page_index: 0,
            rect: [150.0, 60.0, 250.0, 80.0],
            target: address.clone(),
            look: crate::link::Look::default(),
        },
        |after, credential, lock| {
            let links = crate::link::links_of(after, credential, 0).expect("the links read");
            assert_eq!(links.len(), 2, "{lock:?}");
            assert!(
                links
                    .iter()
                    .any(|(_, _, target)| target.as_ref() == Some(&address)),
                "{lock:?}: the new link goes where it was sent: {links:?}"
            );
        },
    );
    under_every_lock(
        &a_document(),
        |_, _| Command::SetLinkProperties {
            page_index: 0,
            link: widget(6),
            target: Some(crate::link::Target::Page(
                1,
                crate::link::Arrival::InheritZoom,
            )),
            look: None,
        },
        |after, credential, lock| {
            let links = crate::link::links_of(after, credential, 0).expect("the links read");
            assert!(
                matches!(links[0].2, Some(crate::link::Target::Page(1, _))),
                "{lock:?}: {links:?}"
            );
        },
    );
    under_every_lock(
        &a_document(),
        |_, _| Command::RemoveLink {
            page_index: 0,
            link: widget(6),
        },
        |after, credential, lock| {
            assert!(
                crate::link::links_of(after, credential, 0)
                    .expect("the links read")
                    .is_empty(),
                "{lock:?}"
            );
        },
    );
}

#[test]
fn a_bookmark_and_a_name_are_added() {
    under_every_lock(
        &a_document(),
        |_, _| Command::ChangeOutline {
            page_index: 0,
            change: crate::outline::Change::Add {
                title: "ບົດທີ 2".to_owned(),
                page: 1,
                after: None,
                inside: false,
            },
        },
        |after, credential, lock| {
            let titles: Vec<(String, Option<usize>)> =
                crate::outline::read_outline(after, credential)
                    .expect("the outline reads")
                    .into_iter()
                    .map(|bookmark| (bookmark.title, bookmark.page))
                    .collect();
            assert_eq!(
                titles,
                vec![("Start".to_owned(), Some(0)), ("ບົດທີ 2".to_owned(), Some(1))],
                "{lock:?}"
            );
        },
    );
    under_every_lock(
        &a_document(),
        |_, _| Command::ChangeNaming {
            page_index: 0,
            change: crate::destination::Naming::Name {
                name: "second".to_owned(),
                page: 1,
                arrival: crate::link::Arrival::InheritZoom,
            },
        },
        |after, credential, lock| {
            let spots = crate::destination::spots_of(after, credential).expect("the names read");
            assert!(
                spots
                    .iter()
                    .any(|spot| spot.name == "second" && spot.page == 1),
                "{lock:?}: {spots:?}"
            );
        },
    );
}

#[test]
fn text_a_drawing_a_watermark_and_a_text_layer_are_put_on_a_page() {
    under_every_lock(
        &a_document(),
        |_, _| Command::PlaceNewText {
            paragraph: crate::plan::ParagraphLayout::default(),
            page_index: 0,
            frame: [150.0, 250.0, 280.0, 270.0],
            text: "AB".to_owned(),
            family: "Test Face".to_owned(),
            size: 10.0,
            bold: false,
            italic: false,
            fill: None,
        },
        |after, credential, lock| {
            assert!(atoms(after, credential, 0) > 1, "{lock:?}");
        },
    );
    under_every_lock(
        &a_document(),
        |_, _| Command::DrawPath {
            page_index: 0,
            steps: vec![PenStep::Move((100.0, 100.0)), PenStep::Line((200.0, 150.0))],
            closed: false,
            stroke: Some(PenStroke::pen([1.0, 0.0, 0.0], 2.0)),
            fill: None,
        },
        |after, credential, lock| {
            assert_eq!(atoms(after, credential, 0), 2, "{lock:?}");
        },
    );
    under_every_lock(
        &a_document(),
        |_, _| Command::Stamp {
            page_index: 0,
            stamp: crate::stamp::Stamp {
                wording: "AB".to_owned(),
                spot: crate::stamp::Spot::Middle,
                family: "Test Face".to_owned(),
                size: 40.0,
                bold: false,
                italic: false,
                fill: Some([0.8, 0.1, 0.1]),
                opacity: 0.3,
                margin: 20.0,
            },
            facts: crate::stamp::Facts {
                number: 1,
                count: 2,
                name: "locked.pdf".to_owned(),
                today: "25/09/2026".to_owned(),
            },
            share_from: None,
        },
        |after, credential, lock| {
            assert!(
                atoms(after, credential, 0) > 1,
                "{lock:?}: the watermark is drawn"
            );
        },
    );
    under_every_lock(
        &a_document(),
        |_, _| Command::TextLayer {
            page_index: 1,
            layer: crate::text_layer::TextLayer {
                words: vec![crate::text_layer::LayerWord {
                    text: "Hello".to_owned(),
                    frame: [50.0, 60.0, 90.0, 72.0],
                }],
            },
            share_from: None,
        },
        |after, credential, lock| {
            assert!(
                atoms(after, credential, 1) > 1,
                "{lock:?}: the recognised word is on the page"
            );
        },
    );
}

#[test]
fn pages_are_turned_duplicated_and_brought_in() {
    under_every_lock(
        &a_document(),
        |_, _| Command::RotatePages {
            pages: vec![1],
            quarter_turns: 1,
        },
        |after, credential, lock| {
            let geometries = pdf_content::page_geometries_with_password(
                after,
                pdf_content::PageContentLimits::default(),
                credential,
            )
            .expect("the pages read");
            assert_eq!(geometries[1].rotate, 90, "{lock:?}");
        },
    );
    under_every_lock(
        &a_document(),
        |locked, lock| Command::InsertPages {
            beside: 1,
            before: false,
            document: Arc::from(locked.to_vec()),
            password: Password(lock.credential().to_vec()),
            pages: vec![0, 1],
        },
        |after, credential, lock| {
            assert_eq!(page_count(after, credential), 4, "{lock:?}");
            assert_eq!(
                atoms(after, credential, 2),
                atoms(after, credential, 0),
                "{lock:?}: the copy paints what its original paints"
            );
            let copied = crate::link::links_of(after, credential, 2).expect("the links read");
            assert!(
                matches!(
                    copied.first().and_then(|(_, _, target)| target.as_ref()),
                    Some(crate::link::Target::Address(address)) if address == "https://example.org/"
                ),
                "{lock:?}: {copied:?}"
            );
        },
    );
    let other = crate::paste::tests::three_kinds();
    for from in [Locked::Plain, Locked::UserAes, Locked::UserRc4] {
        let theirs = from.lock(&other);
        under_every_lock(
            &a_document(),
            |_, _| Command::InsertPages {
                beside: 0,
                before: true,
                document: Arc::from(theirs.to_vec()),
                password: Password(from.credential().to_vec()),
                pages: vec![0],
            },
            |after, credential, lock| {
                assert_eq!(page_count(after, credential), 3, "{from:?} into {lock:?}");
                let text: usize = read_page(after, 0, credential, None)
                    .expect("the page reads")
                    .graph
                    .atoms
                    .iter()
                    .filter(|atom| matches!(atom.kind, pdf_paint::PaintAtomKind::Text(_)))
                    .count();
                assert_eq!(text, 1, "{from:?} into {lock:?}: its text came along");
            },
        );
    }
}

fn a_palette_page() -> ByteStore {
    crate::new_field::tests::document(&[
        "<< /Type /Catalog /Pages 2 0 R >>",
        "<< /Type /Pages /MediaBox [0 0 200 200] /Kids [3 0 R] /Count 1 >>",
        "<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /XObject << /Im1 5 0 R >> >> >>",
        "<< /Length 30 >>\nstream\nq 40 0 0 20 10 10 cm /Im1 Do Q\nendstream",
        "<< /Type /XObject /Subtype /Image /Width 2 /Height 1 /BitsPerComponent 8 \
         /ColorSpace [/Indexed /DeviceRGB 1 <ff000000ff00>] /Length 2 >>\nstream\n\u{0}\u{1}\nendstream",
    ])
}

fn palettes(after: &ByteStore, credential: &[u8], page: usize) -> Vec<Vec<u8>> {
    read_page(after, page, credential, None)
        .expect("the page reads")
        .graph
        .atoms
        .iter()
        .filter_map(|atom| match &atom.kind {
            pdf_paint::PaintAtomKind::Image(image) => match &image.color_space {
                Some(space) => match &space.value {
                    pdf_paint::ColorSpace::Indexed(indexed) => Some(indexed.lookup.value.to_vec()),
                    _ => None,
                },
                None => None,
            },
            _ => None,
        })
        .collect()
}

#[test]
fn a_picture_s_own_palette_is_carried_with_its_page() {
    let wanted = vec![vec![0xff, 0, 0, 0, 0xff, 0]];
    under_every_lock(
        &a_palette_page(),
        |locked, lock| Command::InsertPages {
            beside: 0,
            before: false,
            document: Arc::from(locked.to_vec()),
            password: Password(lock.credential().to_vec()),
            pages: vec![0],
        },
        |after, credential, lock| {
            assert_eq!(
                palettes(after, credential, 0),
                wanted,
                "{lock:?}: the original"
            );
            assert_eq!(palettes(after, credential, 1), wanted, "{lock:?}: the copy");
        },
    );
    for from in [Locked::UserAes, Locked::UserRc4] {
        let theirs = from.lock(&a_palette_page());
        under_every_lock(
            &a_document(),
            |_, _| Command::InsertPages {
                beside: 0,
                before: true,
                document: Arc::from(theirs.to_vec()),
                password: Password(from.credential().to_vec()),
                pages: vec![0],
            },
            |after, credential, lock| {
                assert_eq!(
                    palettes(after, credential, 0),
                    wanted,
                    "{from:?} into {lock:?}"
                );
            },
        );
    }
}

#[test]
fn pages_of_a_locked_file_need_its_password() {
    let theirs = Locked::UserRc4.lock(&crate::paste::tests::three_kinds());
    for password in [b"".as_slice(), b"wrong"] {
        let refused = edited(
            &a_document(),
            Locked::Plain,
            &Command::InsertPages {
                beside: 0,
                before: false,
                document: Arc::from(theirs.to_vec()),
                password: Password(password.to_vec()),
                pages: vec![0],
            },
        );
        assert!(
            refused
                .as_ref()
                .is_err_and(|error| error.to_string().contains("password")),
            "{:?}",
            refused.map(|_| ())
        );
    }
}

#[test]
fn a_copy_is_pasted_in_its_own_document_and_into_another() {
    let source = crate::paste::tests::three_kinds();
    for lock in EVERY_LOCK {
        let locked = lock.lock(&source);
        let credential = lock.credential();
        let read = read_page(&locked, 0, credential, None).expect("the page reads");
        let copied = crate::copy_from(
            &read.graph,
            &crate::paste::tests::anchors(&read.graph),
            locked.id(),
        )
        .expect("it copies");
        let after = edited(
            &locked,
            lock,
            &Command::PasteObjects {
                page_index: 0,
                copied: copied.clone(),
                dx: 0.0,
                dy: -40.0,
                elsewhere: None,
            },
        )
        .unwrap_or_else(|error| panic!("{lock:?}: pasted: {error}"));
        assert_eq!(
            atoms(&after, credential, 0),
            2 * read.graph.atoms.len(),
            "{lock:?}"
        );
        for into in EVERY_LOCK {
            let bare = into.lock(&crate::paste::tests::a_bare_page());
            let after = edited(
                &bare,
                into,
                &Command::PasteObjects {
                    page_index: 0,
                    copied: copied.clone(),
                    dx: 0.0,
                    dy: 0.0,
                    elsewhere: Some((locked.clone(), Password(credential.to_vec()))),
                },
            )
            .unwrap_or_else(|error| panic!("{lock:?} into {into:?}: pasted: {error}"));
            assert_eq!(
                atoms(&after, into.credential(), 0),
                read.graph.atoms.len(),
                "{lock:?} into {into:?}"
            );
        }
    }
}

fn production_lines(text: &str) -> Vec<(usize, &str)> {
    let lines: Vec<&str> = text.lines().collect();
    let mut kept = Vec::new();
    let mut at = 0;
    while at < lines.len() {
        if lines[at].trim_start().starts_with("#[cfg(test)]") {
            let (mut depth, mut opened) = (0_i64, false);
            at += 1;
            while at < lines.len() {
                let line = lines[at];
                let opening = i64::try_from(line.matches('{').count()).unwrap_or(0);
                let closing = i64::try_from(line.matches('}').count()).unwrap_or(0);
                depth += opening - closing;
                opened |= opening > 0;
                at += 1;
                if (opened && depth <= 0) || (!opened && line.trim_end().ends_with(';')) {
                    break;
                }
            }
            continue;
        }
        kept.push((at + 1, lines[at]));
        at += 1;
    }
    kept
}

fn empty_passwords(text: &str) -> Vec<usize> {
    production_lines(text)
        .into_iter()
        .filter(|(_, line)| !line.trim_start().starts_with("//") && line.contains("b\"\""))
        .map(|(number, _)| number)
        .collect()
}

#[test]
fn the_search_for_an_empty_password_finds_one_and_only_one() {
    let text = "fn plan() {\n    read(source, b\"\");\n}\n\
                #[cfg(test)]\nfn helper() {\n    read(source, b\"\");\n}\n\
                #[cfg(test)]\nmod tests {\n    fn a() {\n        read(source, b\"\");\n    }\n}\n\
                // read(source, b\"\") in a comment\n";
    assert_eq!(empty_passwords(text), vec![2]);
}

#[test]
fn no_planner_reads_with_the_empty_password() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut pending = vec![root];
    let mut found = Vec::new();
    while let Some(folder) = pending.pop() {
        for entry in std::fs::read_dir(&folder).expect("the sources are there") {
            let path = entry.expect("an entry").path();
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("");
            if path.is_dir() {
                pending.push(path);
            } else if path.extension().is_some_and(|extension| extension == "rs")
                && !name.ends_with("tests.rs")
            {
                let text = std::fs::read_to_string(&path).expect("a source reads");
                for line in empty_passwords(&text) {
                    found.push(format!("{}:{line}", path.display()));
                }
            }
        }
    }
    assert!(
        found.is_empty(),
        "an empty password where the document's credential belongs: {found:#?}"
    );
}
