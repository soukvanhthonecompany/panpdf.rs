use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use pdf_content::{FontProvider, FontRequest, GlyphProgram, SubstitutedFace, SubstitutionReason};

use super::{Attempt, ResourceIdentity, SCHEMA, census_of, json_lines};

fn face_covering(characters: &[char]) -> Arc<GlyphProgram> {
    let glyph_count = u16::try_from(characters.len() + 1).expect("glyph count");
    let outline = triangle();
    let mut glyf = Vec::new();
    let mut loca = vec![0_u16, 0];
    for _ in characters {
        glyf.extend_from_slice(&outline);
        loca.push(u16::try_from(glyf.len() / 2).expect("loca"));
    }
    let pairs: Vec<(u16, u16)> = characters
        .iter()
        .enumerate()
        .map(|(index, character)| {
            (
                u16::try_from(u32::from(*character)).expect("BMP character"),
                u16::try_from(index + 1).expect("glyph index"),
            )
        })
        .collect();
    let cmap = cmap_table(&pairs);
    let mut head = vec![0_u8; 54];
    head[18..20].copy_from_slice(&1000_u16.to_be_bytes());
    head[50..52].copy_from_slice(&0_u16.to_be_bytes());
    let mut maxp = vec![0_u8; 6];
    maxp[0..4].copy_from_slice(&0x0001_0000_u32.to_be_bytes());
    maxp[4..6].copy_from_slice(&glyph_count.to_be_bytes());
    let loca_bytes: Vec<u8> = loca.iter().flat_map(|value| value.to_be_bytes()).collect();
    let tables: Vec<(&[u8; 4], Vec<u8>)> = vec![
        (b"cmap", cmap),
        (b"glyf", glyf),
        (b"head", head),
        (b"loca", loca_bytes),
        (b"maxp", maxp),
    ];
    Arc::new(GlyphProgram::parse(sfnt(&tables)).expect("the fixture face parses"))
}

fn triangle() -> Vec<u8> {
    let mut glyf = Vec::new();
    glyf.extend_from_slice(&1_i16.to_be_bytes());
    for value in [0_i16, 0, 100, 100] {
        glyf.extend_from_slice(&value.to_be_bytes());
    }
    glyf.extend_from_slice(&2_u16.to_be_bytes());
    glyf.extend_from_slice(&0_u16.to_be_bytes());
    glyf.extend_from_slice(&[0x37, 0x37, 0x37]);
    glyf.extend_from_slice(&[0, 100, 0]);
    glyf.extend_from_slice(&[0, 0, 100]);
    if glyf.len() % 2 == 1 {
        glyf.push(0);
    }
    glyf
}

fn cmap_table(pairs: &[(u16, u16)]) -> Vec<u8> {
    let mut segments: Vec<(u16, u16, i16)> = pairs
        .iter()
        .map(|(character, glyph)| {
            (
                *character,
                *character,
                glyph.wrapping_sub(*character).cast_signed(),
            )
        })
        .collect();
    segments.sort_unstable();
    segments.push((0xFFFF, 0xFFFF, 1));
    let count = u16::try_from(segments.len()).expect("segments");
    let mut subtable = Vec::new();
    subtable.extend_from_slice(&4_u16.to_be_bytes());
    subtable.extend_from_slice(&(16 + count * 8).to_be_bytes());
    subtable.extend_from_slice(&0_u16.to_be_bytes());
    subtable.extend_from_slice(&(count * 2).to_be_bytes());
    subtable.extend_from_slice(&[0; 6]);
    for (_, end, _) in &segments {
        subtable.extend_from_slice(&end.to_be_bytes());
    }
    subtable.extend_from_slice(&0_u16.to_be_bytes());
    for (start, _, _) in &segments {
        subtable.extend_from_slice(&start.to_be_bytes());
    }
    for (_, _, delta) in &segments {
        subtable.extend_from_slice(&delta.to_be_bytes());
    }
    for _ in &segments {
        subtable.extend_from_slice(&0_u16.to_be_bytes());
    }
    let mut out = Vec::new();
    out.extend_from_slice(&0_u16.to_be_bytes());
    out.extend_from_slice(&1_u16.to_be_bytes());
    out.extend_from_slice(&3_u16.to_be_bytes());
    out.extend_from_slice(&1_u16.to_be_bytes());
    out.extend_from_slice(&12_u32.to_be_bytes());
    out.extend_from_slice(&subtable);
    out
}

fn sfnt(tables: &[(&[u8; 4], Vec<u8>)]) -> Vec<u8> {
    let count = u16::try_from(tables.len()).expect("table count");
    let mut out = Vec::new();
    out.extend_from_slice(&0x0001_0000_u32.to_be_bytes());
    out.extend_from_slice(&count.to_be_bytes());
    out.extend_from_slice(&[0; 6]);
    let mut offset = 12 + usize::from(count) * 16;
    let mut records = Vec::new();
    let mut body = Vec::new();
    for (tag, bytes) in tables {
        records.push((*tag, offset, bytes.len()));
        body.extend_from_slice(bytes);
        while body.len() % 4 != 0 {
            body.push(0);
        }
        offset = 12 + usize::from(count) * 16 + body.len();
    }
    for (tag, start, length) in records {
        out.extend_from_slice(tag);
        out.extend_from_slice(&0_u32.to_be_bytes());
        out.extend_from_slice(&u32::try_from(start).expect("offset").to_be_bytes());
        out.extend_from_slice(&u32::try_from(length).expect("length").to_be_bytes());
    }
    out.extend_from_slice(&body);
    out
}

#[derive(Debug)]
struct OneFaceProvider {
    program: Arc<GlyphProgram>,
}

impl OneFaceProvider {
    fn face(&self, reason: SubstitutionReason) -> SubstitutedFace {
        SubstitutedFace {
            program: Arc::clone(&self.program),
            identity: Arc::new(pdf_content::FaceIdentity {
                family: "Census Test Face".to_owned(),
                subfamily: "Regular".to_owned(),
                origin: "test:census".to_owned(),
                sha256: "0".repeat(64),
                face_index: 3,
                style: pdf_content::FontStyle::default(),
            }),
            reason,
        }
    }
}

impl FontProvider for OneFaceProvider {
    fn primary_face(&self, _request: &FontRequest) -> Option<SubstitutedFace> {
        Some(self.face(SubstitutionReason::Generic))
    }

    fn fallback_face(&self, _request: &FontRequest, character: char) -> Option<SubstitutedFace> {
        self.program
            .glyph_for_char(character)
            .map(|_| self.face(SubstitutionReason::ScriptCoverage))
    }

    fn description(&self) -> String {
        "census test provider".to_owned()
    }
}

fn fixture() -> Vec<u8> {
    let mut objects: Vec<(u32, Vec<u8>)> = Vec::new();
    let font = |name: &str, extra: &str| {
        format!(
            "<< /Type /Font /Subtype /TrueType /BaseFont /{name} \
             /FirstChar 65 /LastChar 67 /Widths [500 500 500] \
             /FontDescriptor 9 0 R{extra} >>"
        )
        .into_bytes()
    };
    objects.push((1, b"<< /Type /Catalog /Pages 2 0 R >>".to_vec()));
    objects.push((
        2,
        b"<< /Type /Pages /MediaBox [0 0 200 100] \
           /Kids [10 0 R 11 0 R 12 0 R 13 0 R] /Count 4 >>"
            .to_vec(),
    ));
    objects.push((5, font("Shared", "")));
    objects.push((6, font("Twin", "")));
    objects.push((7, font("Twin", "")));
    objects.push((8, font("Cluster", " /ToUnicode 14 0 R")));
    objects.push((
        9,
        b"<< /Type /FontDescriptor /FontName /Nowhere /Flags 32 /ItalicAngle 0 >>".to_vec(),
    ));
    objects.push((
        10,
        b"<< /Type /Page /Parent 2 0 R /Contents 15 0 R /Resources \
           << /Font << /FA 5 0 R /FB 6 0 R >> >> >>"
            .to_vec(),
    ));
    objects.push((
        11,
        b"<< /Type /Page /Parent 2 0 R /Contents 16 0 R /Resources \
           << /Font << /FA 5 0 R /FB 7 0 R >> >> >>"
            .to_vec(),
    ));
    objects.push((
        12,
        b"<< /Type /Page /Parent 2 0 R /Contents 17 0 R /Resources \
           << /XObject << /X0 20 0 R >> >> >>"
            .to_vec(),
    ));
    objects.push((
        13,
        b"<< /Type /Page /Parent 2 0 R /Contents 9 0 R /Resources << >> >>".to_vec(),
    ));
    objects.push((22, font("Uncovered", " /ToUnicode 23 0 R")));
    objects.push((
        23,
        stream(
            b"begincmap 1 begincodespacerange <00> <FF> endcodespacerange \
              1 beginbfchar <41> <0E01> endbfchar endcmap",
        ),
    ));
    objects.push((
        14,
        stream(
            b"begincmap 1 begincodespacerange <00> <FF> endcodespacerange \
              1 beginbfchar <41> <00410301> endbfchar endcmap",
        ),
    ));
    objects.push((15, stream(b"BT /FA 12 Tf (AB) Tj /FB 12 Tf (C) Tj ET")));
    objects.push((16, stream(b"BT /FA 12 Tf (A) Tj /FB 12 Tf (BC) Tj ET")));
    objects.push((17, stream(b"/X0 Do")));
    objects.push((
        20,
        form(
            b"/X1 Do BT /FD 12 Tf (A) Tj ET",
            b"<< /XObject << /X1 21 0 R >> /Font << /FD 22 0 R >> >>",
        ),
    ));
    objects.push((
        21,
        form(b"BT /FC 12 Tf (A) Tj ET", b"<< /Font << /FC 8 0 R >> >>"),
    ));
    assemble(&objects)
}

fn stream(body: &[u8]) -> Vec<u8> {
    let mut out = format!("<< /Length {} >>\nstream\n", body.len()).into_bytes();
    out.extend_from_slice(body);
    out.extend_from_slice(b"\nendstream");
    out
}

fn form(body: &[u8], resources: &[u8]) -> Vec<u8> {
    let mut out = format!(
        "<< /Type /XObject /Subtype /Form /BBox [0 0 200 100] /Resources {} /Length {} >>\nstream\n",
        String::from_utf8_lossy(resources),
        body.len()
    )
    .into_bytes();
    out.extend_from_slice(body);
    out.extend_from_slice(b"\nendstream");
    out
}

fn assemble(objects: &[(u32, Vec<u8>)]) -> Vec<u8> {
    let highest = objects.iter().map(|(number, _)| *number).max().unwrap_or(0);
    let mut bytes = b"%PDF-1.7\n".to_vec();
    let mut offsets: BTreeMap<u32, usize> = BTreeMap::new();
    for (number, body) in objects {
        offsets.insert(*number, bytes.len());
        bytes.extend_from_slice(format!("{number} 0 obj\n").as_bytes());
        bytes.extend_from_slice(body);
        bytes.extend_from_slice(b"\nendobj\n");
    }
    let size = highest + 1;
    let start = bytes.len();
    bytes.extend_from_slice(format!("xref\n0 {size}\n0000000000 65535 f \n").as_bytes());
    for number in 1..size {
        match offsets.get(&number) {
            Some(offset) => {
                bytes.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
            }
            None => bytes.extend_from_slice(b"0000000000 65535 f \n"),
        }
    }
    bytes.extend_from_slice(
        format!("trailer\n<< /Size {size} /Root 1 0 R >>\nstartxref\n{start}\n%%EOF\n").as_bytes(),
    );
    bytes
}

fn run() -> super::DocumentCensus {
    let provider = Arc::new(OneFaceProvider {
        program: face_covering(&['A', 'B', 'C', '\u{301}']),
    });
    census_of(
        Path::new("census-fixture.pdf"),
        &fixture(),
        usize::MAX,
        b"",
        Some(provider),
    )
}

fn resource(census: &super::DocumentCensus, object: u32) -> &super::Resource {
    census
        .resources
        .get(&ResourceIdentity::Indirect {
            object,
            generation: 0,
        })
        .unwrap_or_else(|| panic!("object {object} is a font resource of the fixture"))
}

#[test]
fn two_resources_with_one_base_font_name_stay_two_resources() {
    let census = run();
    let first = resource(&census, 6);
    let second = resource(&census, 7);
    assert_eq!(first.facts.base_font_display, "/Twin");
    assert_eq!(second.facts.base_font_display, "/Twin");
    assert_eq!(
        first.facts.base_font_hex, second.facts.base_font_hex,
        "the fixture depends on the two names being the same bytes"
    );
    assert_eq!(first.counts.usages, 1, "page 1 shows one run through 6 0 R");
    assert_eq!(first.counts.source_codes, 1);
    assert_eq!(
        second.counts.usages, 1,
        "page 2 shows one run through 7 0 R"
    );
    assert_eq!(second.counts.source_codes, 2);
    assert_eq!(first.pages, [0].into_iter().collect());
    assert_eq!(second.pages, [1].into_iter().collect());
}

#[test]
fn one_resource_used_on_two_pages_stays_one_resource() {
    let census = run();
    let shared = resource(&census, 5);
    assert_eq!(shared.pages, [0, 1].into_iter().collect());
    assert_eq!(shared.counts.usages, 2, "one run on each page");
    assert_eq!(
        shared.counts.source_codes, 3,
        "two codes on page 1 and one on page 2"
    );
    assert_eq!(shared.usages.len(), 2);
    assert_eq!(shared.usages[0].page, 0);
    assert_eq!(shared.usages[0].source_codes, 2);
    assert_eq!(shared.usages[1].page, 1);
    assert_eq!(shared.usages[1].source_codes, 1);
}

#[test]
fn a_resource_inside_nested_forms_records_the_path_it_was_reached_through() {
    let census = run();
    let inner = resource(&census, 8);
    assert_eq!(inner.pages, [2].into_iter().collect());
    assert_eq!(inner.scopes.len(), 1);
    let scope = inner.scopes.iter().next().expect("one scope");
    assert_eq!(
        scope.forms,
        vec![(20, 0), (21, 0)],
        "outer Form then inner Form, outermost first"
    );
    assert_eq!(
        scope.text(),
        "stream 21 0 R > form 20 0 R > form 21 0 R",
        "the stream the operator was in, and the path that reached it"
    );
}

#[test]
fn a_page_that_cannot_be_read_is_counted_with_its_status() {
    let census = run();
    assert_eq!(census.pages_declared, Some(4));
    assert_eq!(census.page_attempts.len(), 4, "every page is attempted");
    assert_eq!(census.pages_read(), 3);
    assert_eq!(census.pages_failed(), 1);
    let (page, attempt) = census
        .page_attempts
        .iter()
        .find(|(_, attempt)| *attempt != Attempt::Success)
        .expect("one page fails");
    assert_eq!(*page, 3);
    assert_ne!(attempt.label(), "success");
    assert!(
        !attempt.detail().is_empty(),
        "a failed attempt has to say what stopped it"
    );
}

#[test]
fn a_cluster_of_several_code_points_is_one_cluster_and_several_outlines() {
    let census = run();
    let cluster = resource(&census, 8);
    assert_eq!(cluster.counts.source_codes, 1, "the page showed one code");
    assert_eq!(cluster.counts.clusters_drawn, 1, "a base and its mark");
    assert_eq!(cluster.counts.outlines_drawn, 2, "two outlines, one origin");
    assert_eq!(
        cluster.counts.routes.get("to-unicode"),
        Some(&1),
        "the file's own `/ToUnicode` established the meaning"
    );
    assert!(cluster.counts.unresolved.is_empty());
    assert_eq!(cluster.facts.to_unicode, "present");
    assert_eq!(cluster.facts.to_unicode_declared, 1);
}

#[test]
fn a_resource_records_its_identity_and_the_face_that_answered_it() {
    let census = run();
    let shared = resource(&census, 5);
    assert_eq!(shared.identity.text(), "5 0 R");
    assert_eq!(shared.facts.face_family, "Census Test Face");
    assert_eq!(shared.facts.face_index, 3, "which face of the file");
    assert_eq!(shared.facts.face_sha256, "0".repeat(64));
    assert_eq!(shared.facts.program_evidence, "not embedded");
    assert!(
        shared.facts.program_sha256.is_empty(),
        "no program was embedded, so there is nothing to hash"
    );
}

#[test]
fn every_source_code_lands_in_exactly_one_column() {
    let census = run();
    for resource in census.resources.values() {
        let counts = &resource.counts;
        let routed: u64 = counts.routes.values().sum();
        assert_eq!(
            routed + counts.unresolved_without_a_route,
            counts.substituted_codes,
            "substituted columns must account for every substituted code of {}",
            resource.identity.text()
        );
        assert!(
            counts.unresolved_without_a_route <= counts.unresolved.values().sum::<u64>(),
            "the routeless part cannot exceed the whole"
        );
        assert_eq!(
            counts.native_glyph_index + counts.native_no_glyph_index,
            counts.native_codes,
            "native columns must account for every native code of {}",
            resource.identity.text()
        );
        assert_eq!(
            counts.native_codes
                + counts.substituted_codes
                + counts.type3_codes
                + counts.undrawable_codes,
            counts.source_codes,
            "the three journeys must account for every code of {}",
            resource.identity.text()
        );
    }
}

#[test]
fn a_code_with_a_meaning_no_face_covers_is_in_two_columns_and_still_adds_up() {
    let census = run();
    let uncovered = resource(&census, 22);
    let counts = &uncovered.counts;
    assert_eq!(counts.substituted_codes, 1);
    assert_eq!(
        counts.routes.get("to-unicode"),
        Some(&1),
        "the meaning is known"
    );
    assert_eq!(
        counts.unresolved.get("no face coverage"),
        Some(&1),
        "and nothing drew it"
    );
    assert_eq!(
        counts.unresolved_without_a_route, 0,
        "this code did reach a route, so it is not part of the routeless count"
    );
    assert_eq!(counts.uncovered.get(&0x0E01), Some(&1));
    assert_eq!(counts.clusters_drawn, 0);
    assert_eq!(counts.outlines_drawn, 0);

    let routed: u64 = counts.routes.values().sum();
    let unresolved: u64 = counts.unresolved.values().sum();
    assert_eq!(
        routed + unresolved,
        2,
        "the sum a careless reader would take"
    );
    assert_eq!(
        routed + counts.unresolved_without_a_route,
        1,
        "the code count"
    );
}

#[test]
fn the_checkpoint_lines_name_the_schema_and_replay_as_completed_work() {
    let census = run();
    let lines = json_lines(&census);
    assert_eq!(
        lines
            .iter()
            .filter(|line| line.contains(r#""record":"file""#))
            .count(),
        1
    );
    assert_eq!(
        lines
            .iter()
            .filter(|line| line.contains(r#""record":"page""#))
            .count(),
        census.page_attempts.len(),
        "every attempted page gets a record"
    );
    assert_eq!(
        lines
            .iter()
            .filter(|line| line.contains(r#""record":"resource""#))
            .count(),
        census.resources.len()
    );
    assert!(lines.join("\n").contains(SCHEMA));
}
