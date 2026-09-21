use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::Path;
use std::sync::Arc;

use pdf_bytes::{ByteStore, SourceId};
use pdf_content::{FontProvider, FontRequest, ProgramEvidence, SUBSTITUTION_POLICY};
use pdf_paint::{FormInvocation, PaintAtom, PaintAtomKind, PaintId, PatternInvocation};

pub const SCHEMA: &str = "panpdf.font-census/2";

pub const MAX_RECORDED_USAGES: usize = 8;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Attempt {
    Success,
    ParseRefusal(String),
    PasswordRequired,
    ResourceLimit(String),
    Timeout,
    Crash(String),
    ReferenceFailure(String),
}

impl Attempt {
    #[must_use]
    pub fn label(&self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::ParseRefusal(_) => "parse_refusal",
            Self::PasswordRequired => "password_required",
            Self::ResourceLimit(_) => "resource_limit",
            Self::Timeout => "timeout",
            Self::Crash(_) => "crash",
            Self::ReferenceFailure(_) => "reference_failure",
        }
    }

    #[must_use]
    pub fn detail(&self) -> &str {
        match self {
            Self::Success | Self::PasswordRequired | Self::Timeout => "",
            Self::ParseRefusal(text)
            | Self::ResourceLimit(text)
            | Self::Crash(text)
            | Self::ReferenceFailure(text) => text,
        }
    }

    #[must_use]
    pub fn of_page_content_error(error: &pdf_content::PageContentError) -> Self {
        Self::of_text(&error.to_string())
    }

    #[must_use]
    pub fn of_page_error(error: &pdf_session::PageError) -> Self {
        Self::of_text(&error.to_string())
    }

    fn of_text(text: &str) -> Self {
        let text = text.to_owned();
        let lowered = text.to_ascii_lowercase();
        if lowered.contains("password") || lowered.contains("credential") {
            return Self::PasswordRequired;
        }
        if lowered.contains("limit") || lowered.contains("too many") || lowered.contains("exceed") {
            return Self::ResourceLimit(text);
        }
        if lowered.contains("resolve") || lowered.contains("reference") {
            return Self::ReferenceFailure(text);
        }
        Self::ParseRefusal(text)
    }
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum ResourceIdentity {
    Indirect {
        object: u32,
        generation: u16,
    },
    Direct {
        source: u64,
        derivation: u64,
        start: usize,
        end: usize,
    },
    Unnamed,
}

impl ResourceIdentity {
    #[must_use]
    pub fn text(&self) -> String {
        match self {
            Self::Indirect { object, generation } => format!("{object} {generation} R"),
            Self::Direct {
                source,
                derivation,
                start,
                end,
            } => format!("span:{source}:{derivation}:{start}..{end}"),
            Self::Unnamed => "unnamed".to_owned(),
        }
    }
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct Scope {
    pub stream: (u32, u16),
    pub forms: Vec<(u32, u16)>,
    pub patterns: Vec<(u32, u16)>,
}

impl Scope {
    fn of(id: &PaintId) -> Self {
        Self {
            stream: (id.stream.object_number(), id.stream.generation()),
            forms: id
                .invocation_path
                .iter()
                .map(|FormInvocation { form, .. }| (form.object_number(), form.generation()))
                .collect(),
            patterns: id
                .pattern_path
                .iter()
                .map(|PatternInvocation { pattern, .. }| {
                    (pattern.object_number(), pattern.generation())
                })
                .collect(),
        }
    }

    #[must_use]
    pub fn text(&self) -> String {
        let mut out = format!("stream {} {} R", self.stream.0, self.stream.1);
        for (object, generation) in &self.forms {
            let _ = write!(out, " > form {object} {generation} R");
        }
        for (object, generation) in &self.patterns {
            let _ = write!(out, " > pattern {object} {generation} R");
        }
        out
    }
}

#[derive(Clone, Debug)]
pub struct Usage {
    pub page: usize,
    pub scope: Scope,
    pub atom_ordinal: usize,
    pub source_codes: u64,
}

#[derive(Clone, Debug, Default)]
pub struct Counts {
    pub usages: u64,
    pub source_codes: u64,

    pub native_codes: u64,
    pub native_with_cid: u64,
    pub native_glyph_index: u64,
    pub native_no_glyph_index: u64,

    pub substituted_codes: u64,
    pub routes: BTreeMap<&'static str, u64>,
    pub unresolved: BTreeMap<&'static str, u64>,
    pub unresolved_without_a_route: u64,
    pub clusters_drawn: u64,
    pub outlines_drawn: u64,
    pub uncovered: BTreeMap<u32, u64>,

    pub type3_codes: u64,
    pub type3_procedures: u64,

    pub undrawable_codes: u64,
}

#[derive(Clone, Debug, Default)]
pub struct ResourceFacts {
    pub base_font_hex: String,
    pub base_font_display: String,
    pub family: String,
    pub subtype: String,
    pub cid_subtype: String,
    pub registry: String,
    pub ordering: String,
    pub encoding: String,
    pub flags: u32,
    pub weight: u16,
    pub italic: bool,
    pub italic_angle: Option<f64>,
    pub standard_face: String,
    pub program_evidence: String,
    pub program_evidence_detail: String,
    pub program_technology: String,
    pub program_sha256: String,
    pub program_bytes: u64,
    pub program_units_per_em: u16,
    pub program_cid_keyed: bool,
    pub to_unicode: String,
    pub to_unicode_declared: usize,
    pub to_unicode_named: usize,
    pub face_family: String,
    pub face_subfamily: String,
    pub face_sha256: String,
    pub face_index: u32,
    pub face_origin: String,
    pub face_reason: String,
    pub fallback_faces: BTreeSet<String>,
    pub unresolved_reason: String,
    pub type3: bool,
}

#[derive(Clone, Debug)]
pub struct Resource {
    pub identity: ResourceIdentity,
    pub facts: ResourceFacts,
    pub counts: Counts,
    pub pages: BTreeSet<usize>,
    pub scopes: BTreeSet<Scope>,
    pub usages: Vec<Usage>,
}

#[derive(Clone, Debug)]
pub struct DocumentCensus {
    pub path: String,
    pub document_sha256: String,
    pub bytes: u64,
    pub pages_declared: Option<usize>,
    pub page_attempts: Vec<(usize, Attempt)>,
    pub file_attempt: Attempt,
    pub structure_repairs: usize,
    pub resources: BTreeMap<ResourceIdentity, Resource>,
}

impl DocumentCensus {
    #[must_use]
    pub fn pages_read(&self) -> usize {
        self.page_attempts
            .iter()
            .filter(|(_, attempt)| *attempt == Attempt::Success)
            .count()
    }

    #[must_use]
    pub fn pages_failed(&self) -> usize {
        self.page_attempts.len() - self.pages_read()
    }
}

#[must_use]
pub fn census_of(
    path: &Path,
    bytes: &[u8],
    limit: usize,
    credential: &[u8],
    provider: Option<Arc<dyn FontProvider>>,
) -> DocumentCensus {
    census_range(path, bytes, 0..limit, credential, provider)
}

#[must_use]
pub fn census_range(
    path: &Path,
    bytes: &[u8],
    pages: std::ops::Range<usize>,
    credential: &[u8],
    provider: Option<Arc<dyn FontProvider>>,
) -> DocumentCensus {
    let document_sha256 = pdf_content::sha256_hex(bytes);
    let mut census = DocumentCensus {
        path: path.display().to_string(),
        document_sha256,
        bytes: bytes.len() as u64,
        pages_declared: None,
        page_attempts: Vec::new(),
        file_attempt: Attempt::Success,
        structure_repairs: 0,
        resources: BTreeMap::new(),
    };
    let source = ByteStore::new(SourceId::new(1), Arc::<[u8]>::from(bytes.to_vec()));
    let count = match pdf_content::count_pages_recovering(
        &source,
        pdf_content::PageContentLimits::default(),
        pdf_content::RecoverLimits::default(),
        credential,
    ) {
        Ok(recovered) => {
            let (count, repairs) = recovered.into_parts();
            census.structure_repairs = repairs.len();
            count
        }
        Err(error) => {
            census.file_attempt = Attempt::of_page_content_error(&error);
            return census;
        }
    };
    census.pages_declared = Some(count);
    let mut session = pdf_session::Session::with_fonts(source, credential, provider);
    for index in pages.start..count.min(pages.end) {
        match session.page(index) {
            Ok(view) => {
                walk(&view.graph.atoms, index, &mut census.resources);
                census.page_attempts.push((index, Attempt::Success));
            }
            Err(error) => {
                census
                    .page_attempts
                    .push((index, Attempt::of_page_error(&error)));
            }
        }
        session.forget_pages();
    }
    census
}

fn walk(atoms: &[PaintAtom], page: usize, resources: &mut BTreeMap<ResourceIdentity, Resource>) {
    for (ordinal, atom) in atoms.iter().enumerate() {
        match &atom.kind {
            PaintAtomKind::TransparencyGroup(group) => walk(&group.graph.atoms, page, resources),
            PaintAtomKind::Text(text) => record(atom, ordinal, text, page, resources),
            _ => {}
        }
    }
}

fn identity_of(text: &pdf_paint::TextShowPaint) -> ResourceIdentity {
    let Some(applied) = text.state.text.font.as_ref() else {
        return ResourceIdentity::Unnamed;
    };
    if let Some(reference) = applied.value.reference {
        return ResourceIdentity::Indirect {
            object: reference.object_number(),
            generation: reference.generation(),
        };
    }
    applied
        .provenance
        .first()
        .map_or(ResourceIdentity::Unnamed, |span| ResourceIdentity::Direct {
            source: span.source().get(),
            derivation: span.source().derivation(),
            start: span.start(),
            end: span.end(),
        })
}

fn record(
    atom: &PaintAtom,
    ordinal: usize,
    text: &pdf_paint::TextShowPaint,
    page: usize,
    resources: &mut BTreeMap<ResourceIdentity, Resource>,
) {
    if text.glyphs.is_empty() {
        return;
    }
    let identity = identity_of(text);
    let entry = resources
        .entry(identity.clone())
        .or_insert_with(|| Resource {
            identity: identity.clone(),
            facts: ResourceFacts::default(),
            counts: Counts::default(),
            pages: BTreeSet::new(),
            scopes: BTreeSet::new(),
            usages: Vec::new(),
        });
    fill_facts(&mut entry.facts, text);
    let scope = Scope::of(&atom.id);
    entry.pages.insert(page);
    entry.scopes.insert(scope.clone());
    let codes = text.glyphs.len() as u64;
    if entry.usages.len() < MAX_RECORDED_USAGES {
        entry.usages.push(Usage {
            page,
            scope,
            atom_ordinal: ordinal,
            source_codes: codes,
        });
    }
    let counts = &mut entry.counts;
    counts.usages += 1;
    counts.source_codes += codes;

    if text.type3 {
        counts.type3_codes += codes;
        counts.type3_procedures += text
            .glyphs
            .iter()
            .filter(|glyph| glyph.procedure.is_some())
            .count() as u64;
        return;
    }
    if text.program.is_some() {
        counts.native_codes += codes;
        for glyph in &text.glyphs {
            if glyph.code.cid.is_some() {
                counts.native_with_cid += 1;
            }
            if glyph.glyph.is_some() {
                counts.native_glyph_index += 1;
            } else {
                counts.native_no_glyph_index += 1;
            }
        }
        return;
    }
    let Some(substitution) = text.substitution.as_ref() else {
        counts.undrawable_codes += codes;
        return;
    };
    counts.substituted_codes += codes;
    let census = &substitution.census;
    for (route, count) in &census.routes {
        *counts.routes.entry(route.label()).or_default() += u64::from(*count);
    }
    for (reason, count) in &census.unresolved {
        *counts.unresolved.entry(reason.label()).or_default() += u64::from(*count);
    }
    counts.unresolved_without_a_route += u64::from(census.codes_without_a_route());
    counts.clusters_drawn += u64::from(census.clusters_drawn);
    counts.outlines_drawn += u64::from(census.outlines_drawn);
    for (character, count) in &substitution.unmapped {
        *counts.uncovered.entry(u32::from(*character)).or_default() += u64::from(*count);
    }
}

fn fill_facts(facts: &mut ResourceFacts, text: &pdf_paint::TextShowPaint) {
    if text.type3 {
        facts.type3 = true;
        if facts.program_evidence.is_empty() {
            "type 3 procedures".clone_into(&mut facts.program_evidence);
        }
    }
    if let Some(program) = text.program.as_ref()
        && facts.program_sha256.is_empty()
    {
        let bytes = program.program_bytes();
        program
            .technology()
            .clone_into(&mut facts.program_technology);
        facts.program_sha256 = pdf_content::sha256_hex(bytes);
        facts.program_bytes = bytes.len() as u64;
        facts.program_units_per_em = program.units_per_em();
        facts.program_cid_keyed = program.is_cid_keyed();
    }
    if facts.to_unicode.is_empty() {
        facts.to_unicode = match text.text.refused() {
            Some(error) => format!("refused: {error}"),
            None if text.text.is_empty() => "absent".to_owned(),
            None => "present".to_owned(),
        };
        facts.to_unicode_declared = text.text.by_confidence(pdf_content::Confidence::Declared);
        facts.to_unicode_named = text.text.by_confidence(pdf_content::Confidence::Named);
    }
    if let Some(substitution) = text.substitution.as_ref() {
        let identity = &substitution.primary.identity;
        if facts.face_sha256.is_empty() {
            facts.face_family.clone_from(&identity.family);
            facts.face_subfamily.clone_from(&identity.subfamily);
            facts.face_sha256.clone_from(&identity.sha256);
            facts.face_index = identity.face_index;
            facts.face_origin.clone_from(&identity.origin);
            facts.face_reason = substitution.primary.reason.to_string();
        }
        for face in &substitution.fallbacks {
            facts.fallback_faces.insert(format!(
                "{} {} {} #{}",
                face.identity.family,
                face.identity.subfamily,
                face.identity.sha256,
                face.identity.face_index
            ));
        }
    }
    let request = text
        .substitution
        .as_ref()
        .map(|substitution| substitution.request.as_ref())
        .or(text.font_request.as_deref());
    let Some(request) = request else {
        if facts.unresolved_reason.is_empty() && !text.type3 && text.program.is_none() {
            "no font request could be read".clone_into(&mut facts.unresolved_reason);
        }
        return;
    };
    if facts.base_font_hex.is_empty() {
        fill_request_facts(facts, request);
    }
    if text.program.is_none() && text.substitution.is_none() && !text.type3 {
        "no face answered this request".clone_into(&mut facts.unresolved_reason);
    }
}

fn fill_request_facts(facts: &mut ResourceFacts, request: &FontRequest) {
    facts.base_font_hex = hex(&request.base_font);
    facts.base_font_display = printable(&request.base_font);
    facts.family.clone_from(&request.family);
    facts.subtype = printable(&request.subtype);
    facts.cid_subtype = request
        .cid_subtype
        .as_ref()
        .map(|value| printable(value))
        .unwrap_or_default();
    facts.registry = request.registry.clone().unwrap_or_default();
    facts.ordering = request.ordering.clone().unwrap_or_default();
    facts.encoding = request.encoding.clone().unwrap_or_default();
    facts.flags = request.flags.0;
    facts.weight = request.style.weight;
    facts.italic = request.style.italic;
    facts.italic_angle = request.italic_angle;
    facts.standard_face = request
        .standard_face
        .map(|face| face.name().to_owned())
        .unwrap_or_default();
    let (label, detail) = match &request.program {
        ProgramEvidence::Unreadable { key, reason } => (
            "embedded but unreadable",
            format!("{}: {reason}", printable(key)),
        ),
        ProgramEvidence::Embedded { key } => ("embedded", printable(key)),
        other => (other.label(), String::new()),
    };
    if facts.program_evidence.is_empty() {
        facts.program_evidence = label.to_owned();
    }
    facts.program_evidence_detail = detail;
}

#[derive(Clone, Debug, Default)]
pub struct Summary {
    pub documents: u64,
    pub documents_failed: u64,
    pub pages_attempted: u64,
    pub pages_read: u64,
    pub page_status: BTreeMap<&'static str, u64>,
    pub file_status: BTreeMap<&'static str, u64>,
    pub resources: u64,
    pub resources_with_program: u64,
    pub resources_substituted: u64,
    pub resources_undrawable: u64,
    pub resources_type3: u64,
    pub source_codes: u64,
    pub native_glyph_index: u64,
    pub native_no_glyph_index: u64,
    pub outlines_drawn: u64,
    pub clusters_drawn: u64,
    pub routes: BTreeMap<&'static str, u64>,
    pub unresolved: BTreeMap<&'static str, u64>,
    pub uncovered: BTreeMap<u32, u64>,
    pub by_family: BTreeMap<String, u64>,
}

impl Summary {
    pub fn add(&mut self, census: &DocumentCensus) {
        self.documents += 1;
        *self
            .file_status
            .entry(census.file_attempt.label())
            .or_default() += 1;
        if census.file_attempt != Attempt::Success {
            self.documents_failed += 1;
        }
        self.pages_attempted += census.page_attempts.len() as u64;
        for (_, attempt) in &census.page_attempts {
            *self.page_status.entry(attempt.label()).or_default() += 1;
            if *attempt == Attempt::Success {
                self.pages_read += 1;
            }
        }
        for resource in census.resources.values() {
            self.resources += 1;
            if resource.facts.type3 {
                self.resources_type3 += 1;
            } else if !resource.facts.program_sha256.is_empty() {
                self.resources_with_program += 1;
            } else if !resource.facts.face_sha256.is_empty() {
                self.resources_substituted += 1;
            } else {
                self.resources_undrawable += 1;
            }
            let counts = &resource.counts;
            self.source_codes += counts.source_codes;
            self.native_glyph_index += counts.native_glyph_index;
            self.native_no_glyph_index += counts.native_no_glyph_index;
            self.outlines_drawn += counts.outlines_drawn;
            self.clusters_drawn += counts.clusters_drawn;
            for (route, count) in &counts.routes {
                *self.routes.entry(route).or_default() += count;
            }
            for (reason, count) in &counts.unresolved {
                *self.unresolved.entry(reason).or_default() += count;
            }
            for (character, count) in &counts.uncovered {
                *self.uncovered.entry(*character).or_default() += count;
            }
            let family = if resource.facts.family.is_empty() {
                resource.facts.base_font_display.clone()
            } else {
                resource.facts.family.clone()
            };
            *self.by_family.entry(family).or_default() += counts.usages;
        }
    }
}

fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

fn printable(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len());
    for byte in bytes {
        if byte.is_ascii_graphic() || *byte == b' ' {
            out.push(char::from(*byte));
        } else {
            let _ = write!(out, "\\x{byte:02x}");
        }
    }
    out
}

#[must_use]
pub fn json_string(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for character in text.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if u32::from(c) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", u32::from(c));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn json_map<K: std::fmt::Display>(map: &BTreeMap<K, u64>) -> String {
    let body: Vec<String> = map
        .iter()
        .map(|(key, value)| format!("{}:{value}", json_string(&key.to_string())))
        .collect();
    format!("{{{}}}", body.join(","))
}

fn json_characters(map: &BTreeMap<u32, u64>) -> String {
    let body: Vec<String> = map
        .iter()
        .map(|(key, value)| format!("\"U+{key:04X}\":{value}"))
        .collect();
    format!("{{{}}}", body.join(","))
}

fn json_numbers(values: impl IntoIterator<Item = usize>) -> String {
    let body: Vec<String> = values.into_iter().map(|value| value.to_string()).collect();
    format!("[{}]", body.join(","))
}

fn json_strings<'a>(values: impl IntoIterator<Item = &'a str>) -> String {
    let body: Vec<String> = values.into_iter().map(json_string).collect();
    format!("[{}]", body.join(","))
}

#[must_use]
pub fn json_lines(census: &DocumentCensus) -> Vec<String> {
    let mut lines = Vec::new();
    lines.push(format!(
        concat!(
            r#"{{"record":"file","schema":{},"document_sha256":{},"path":{},"bytes":{},"#,
            r#""pages_declared":{},"pages_attempted":{},"pages_read":{},"structure_repairs":{},"#,
            r#""status":{},"detail":{}}}"#
        ),
        json_string(SCHEMA),
        json_string(&census.document_sha256),
        json_string(&census.path),
        census.bytes,
        census
            .pages_declared
            .map_or_else(|| "null".to_owned(), |count| count.to_string()),
        census.page_attempts.len(),
        census.pages_read(),
        census.structure_repairs,
        json_string(census.file_attempt.label()),
        json_string(census.file_attempt.detail()),
    ));
    for (page, attempt) in &census.page_attempts {
        lines.push(format!(
            r#"{{"record":"page","document_sha256":{},"page":{page},"status":{},"detail":{}}}"#,
            json_string(&census.document_sha256),
            json_string(attempt.label()),
            json_string(attempt.detail()),
        ));
    }
    for resource in census.resources.values() {
        lines.push(resource_line(census, resource));
    }
    lines
}

#[expect(clippy::too_many_lines, reason = "one record, one field per line")]
fn resource_line(census: &DocumentCensus, resource: &Resource) -> String {
    let facts = &resource.facts;
    let counts = &resource.counts;
    let usages: Vec<String> = resource
        .usages
        .iter()
        .map(|usage| {
            format!(
                r#"{{"page":{},"scope":{},"atom":{},"source_codes":{}}}"#,
                usage.page,
                json_string(&usage.scope.text()),
                usage.atom_ordinal,
                usage.source_codes
            )
        })
        .collect();
    format!(
        concat!(
            r#"{{"record":"resource","document_sha256":{},"identity":{},"#,
            r#""base_font_hex":{},"base_font":{},"family":{},"subtype":{},"#,
            r#""cid_subtype":{},"registry":{},"ordering":{},"encoding":{},"#,
            r#""flags":{},"weight":{},"italic":{},"italic_angle":{},"standard_face":{},"#,
            r#""program_evidence":{},"program_evidence_detail":{},"#,
            r#""program_technology":{},"program_sha256":{},"program_bytes":{},"#,
            r#""program_units_per_em":{},"program_cid_keyed":{},"#,
            r#""to_unicode":{},"to_unicode_declared":{},"to_unicode_named":{},"#,
            r#""policy":{},"face_family":{},"face_subfamily":{},"face_sha256":{},"#,
            r#""face_index":{},"face_origin":{},"face_reason":{},"fallback_faces":{},"#,
            r#""unresolved_reason":{},"type3":{},"#,
            r#""pages":{},"scopes":{},"usages_recorded":{},"usages_capped_at":{},"usages":[{}],"#,
            r#""counts":{{"usages":{},"source_codes":{},"#,
            r#""native_codes":{},"native_with_cid":{},"native_glyph_index":{},"#,
            r#""native_no_glyph_index":{},"substituted_codes":{},"#,
            r#""unresolved_without_a_route":{},"#,
            r#""clusters_drawn":{},"outlines_drawn":{},"#,
            r#""type3_codes":{},"type3_procedures":{},"undrawable_codes":{},"#,
            r#""routes":{},"unresolved":{},"uncovered":{}}}}}"#
        ),
        json_string(&census.document_sha256),
        json_string(&resource.identity.text()),
        json_string(&facts.base_font_hex),
        json_string(&facts.base_font_display),
        json_string(&facts.family),
        json_string(&facts.subtype),
        json_string(&facts.cid_subtype),
        json_string(&facts.registry),
        json_string(&facts.ordering),
        json_string(&facts.encoding),
        facts.flags,
        facts.weight,
        facts.italic,
        facts
            .italic_angle
            .map_or_else(|| "null".to_owned(), |angle| format!("{angle}")),
        json_string(&facts.standard_face),
        json_string(&facts.program_evidence),
        json_string(&facts.program_evidence_detail),
        json_string(&facts.program_technology),
        json_string(&facts.program_sha256),
        facts.program_bytes,
        facts.program_units_per_em,
        facts.program_cid_keyed,
        json_string(&facts.to_unicode),
        facts.to_unicode_declared,
        facts.to_unicode_named,
        json_string(SUBSTITUTION_POLICY),
        json_string(&facts.face_family),
        json_string(&facts.face_subfamily),
        json_string(&facts.face_sha256),
        facts.face_index,
        json_string(&facts.face_origin),
        json_string(&facts.face_reason),
        json_strings(facts.fallback_faces.iter().map(String::as_str)),
        json_string(&facts.unresolved_reason),
        facts.type3,
        json_numbers(resource.pages.iter().copied()),
        json_strings(
            resource
                .scopes
                .iter()
                .map(Scope::text)
                .collect::<Vec<_>>()
                .iter()
                .map(String::as_str)
        ),
        resource.usages.len(),
        MAX_RECORDED_USAGES,
        usages.join(","),
        counts.usages,
        counts.source_codes,
        counts.native_codes,
        counts.native_with_cid,
        counts.native_glyph_index,
        counts.native_no_glyph_index,
        counts.substituted_codes,
        counts.unresolved_without_a_route,
        counts.clusters_drawn,
        counts.outlines_drawn,
        counts.type3_codes,
        counts.type3_procedures,
        counts.undrawable_codes,
        json_map(&counts.routes),
        json_map(&counts.unresolved),
        json_characters(&counts.uncovered),
    )
}

#[cfg(test)]
#[path = "census_tests.rs"]
mod tests;
