use std::borrow::Cow;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt;
use std::sync::Arc;

mod annotations;
mod links;
mod ownership;

pub use annotations::{Annotation, AnnotationFlags, Annotations, Appearance, UnreadableAnnotation};
pub use links::{
    Destination, Followed, Link, LinkAction, LinkResolver, PageLinks, View, open_link_resolver,
    open_link_resolver_recovering, open_link_resolver_tolerating_damage,
};

use pdf_bytes::{ByteStore, SourceId};
use pdf_font::glyph::{GlyphProgram, GlyphProgramError};
use pdf_font::standard14::Standard14;
use pdf_font::substitute::{
    FontFlags, FontRequest, FontStyle, ProgramEvidence, split_family_style,
};
use pdf_font::{
    CMap, CMapError, CMapLimits, CompositeFont, Font, FontError, SimpleFont, ToUnicode,
    Type0Encoding, parse_cid_font, parse_cmap, parse_simple_font, parse_type0_font,
};
use pdf_security::{AuthenticatedSecurity, SecurityErrorKind, authenticate_standard_password};
use std::ops::ControlFlow;

use pdf_syntax::{
    DecryptionRefused, DocumentObjects, HeaderError, ImageCodec, NameDecodeError, NumberKind,
    Object, ObjectKind, RecoverLimits, Recovered, Reference, Repair, ResolveError, ResolveLimits,
    ResolvedObject, RevisionIndex, StreamDecodeError, StreamRepair, XrefError, XrefLimits,
    decode_image_stream_bytes_recovering, decode_stream_bytes, decode_stream_bytes_recovering,
    open_objects_recovering, parse_header_recovering, parse_header_strict,
    parse_revision_chain_strict,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PageContentLimits {
    pub xref: XrefLimits,
    pub resolve: ResolveLimits,
    pub max_page_tree_nodes: usize,
    pub max_page_tree_depth: usize,
    pub max_content_streams: usize,
    pub max_form_depth: usize,
    pub max_form_xobjects: usize,
    pub max_decoded_stream_bytes: usize,
    pub max_type3_procedures: usize,
}

impl Default for PageContentLimits {
    fn default() -> Self {
        Self {
            xref: XrefLimits::default(),
            resolve: ResolveLimits::default(),
            max_page_tree_nodes: 1_000_000,
            max_page_tree_depth: 1_024,
            max_content_streams: 16_384,
            max_form_depth: 128,
            max_form_xobjects: 100_000,
            max_decoded_stream_bytes: 256 * 1024 * 1024,
            max_type3_procedures: 65_536,
        }
    }
}

#[derive(Clone, Debug)]
pub struct DecodedContentStream {
    pub reference: Reference,
    pub bytes: ByteStore,
    pub repairs: Vec<StreamRepair>,
}

#[derive(Clone, Debug)]
pub struct PageProgram {
    pub page: Reference,
    pub geometry: PageGeometry,
    pub streams: Vec<DecodedContentStream>,
    pub resources: PageResources,
    pub annotations: Annotations,
    ownership: Arc<ownership::ContentOwnership>,
}

impl PageProgram {
    #[must_use]
    pub fn may_modify_content(&self) -> bool {
        self.resources
            .binding
            .as_ref()
            .and_then(|binding| binding.security.as_ref())
            .is_none_or(|security| security.may_modify_content())
    }

    #[must_use]
    pub fn print_allowance(&self) -> pdf_security::PrintAllowance {
        self.resources
            .binding
            .as_ref()
            .and_then(|binding| binding.security.as_ref())
            .map_or(pdf_security::PrintAllowance::Faithful, |security| {
                security.print_allowance()
            })
    }

    pub fn content_stream_is_exclusive(
        &self,
        reference: Reference,
    ) -> Result<bool, PageContentError> {
        self.ownership.is_exclusive(reference)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PageGeometry {
    pub media_box: [f64; 4],
    pub media_box_span: pdf_bytes::SourceSpan,
    pub crop_box: [f64; 4],
    pub crop_box_span: Option<pdf_bytes::SourceSpan>,
    pub rotate: u16,
    pub rotate_span: Option<pdf_bytes::SourceSpan>,
}

impl PageGeometry {
    #[must_use]
    pub fn rotated_size(&self) -> (f64, f64) {
        let width = self.crop_box[2] - self.crop_box[0];
        let height = self.crop_box[3] - self.crop_box[1];
        if matches!(self.rotate, 90 | 270) {
            (height, width)
        } else {
            (width, height)
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct PageResources {
    optional_content: Arc<OptionalContent>,
    ext_gstates: Vec<ResourceEntry>,
    xobjects: Vec<ResourceEntry>,
    fonts: Vec<ResourceEntry>,
    color_spaces: Vec<ResourceEntry>,
    patterns: Vec<ResourceEntry>,
    shadings: Vec<ResourceEntry>,
    properties: Vec<ResourceEntry>,
    binding: Option<Box<ResourceBinding>>,
}

#[derive(Clone, Debug)]
struct ResourceBinding {
    document: ByteStore,
    index: Arc<RevisionIndex>,
    security: Option<Arc<AuthenticatedSecurity>>,
    limits: PageContentLimits,
}

impl PageResources {
    #[must_use]
    pub fn inline_entry(&self, source: ByteStore, value: Object) -> Option<ResourceEntry> {
        let binding = self.binding.as_deref()?;
        Some(ResourceEntry {
            name: b"(inline image)".to_vec(),
            reference: None,
            source,
            value,
            form: None,
            form_error: None,
            document: binding.document.clone(),
            index: Arc::clone(&binding.index),
            security: binding.security.clone(),
            optional_content: Arc::clone(&self.optional_content),
            limits: binding.limits,
        })
    }
    #[must_use]
    pub fn optional_content(&self) -> &OptionalContent {
        &self.optional_content
    }

    #[must_use]
    pub fn ext_gstate(&self, name: &[u8]) -> Option<&ResourceEntry> {
        self.ext_gstates
            .iter()
            .find(|resource| resource.name == name)
    }

    #[must_use]
    pub fn xobject(&self, name: &[u8]) -> Option<&ResourceEntry> {
        self.xobjects.iter().find(|resource| resource.name == name)
    }

    #[must_use]
    pub fn ext_gstates(&self) -> &[ResourceEntry] {
        &self.ext_gstates
    }

    #[must_use]
    pub fn xobjects(&self) -> &[ResourceEntry] {
        &self.xobjects
    }

    #[must_use]
    pub fn with_fonts(&self, fonts: impl IntoIterator<Item = ResourceEntry>) -> Self {
        let mut out = self.clone();
        for font in fonts {
            out.fonts.retain(|had| had.name != font.name);
            out.fonts.push(font);
        }
        out
    }

    #[must_use]
    pub fn font(&self, name: &[u8]) -> Option<&ResourceEntry> {
        self.fonts.iter().find(|resource| resource.name == name)
    }

    pub fn font_written_in(
        &self,
        name: &[u8],
        reference: Reference,
        objects: &ByteStore,
        limits: PageContentLimits,
    ) -> Result<ResourceEntry, PageContentError> {
        let binding = self
            .binding
            .as_deref()
            .ok_or_else(|| PageContentError::new(PageContentErrorKind::MissingRevision))?;
        let chain = parse_revision_chain_strict(objects, limits.xref)
            .map_err(|error| PageContentError::new(PageContentErrorKind::Revisions(error)))?;
        let index = Arc::new(
            RevisionIndex::from_chain(&chain)
                .map_err(|error| PageContentError::new(PageContentErrorKind::Resolve(error)))?,
        );
        let resolved = index
            .resolve_object(objects, reference, limits.resolve)
            .map_err(|error| PageContentError::new(PageContentErrorKind::Resolve(error)))?;
        Ok(ResourceEntry {
            name: name.to_vec(),
            reference: Some(reference),
            source: resolved.source().clone(),
            value: resolved.value().clone(),
            form: None,
            form_error: None,
            document: objects.clone(),
            index,
            security: binding.security.clone(),
            optional_content: Arc::clone(&self.optional_content),
            limits,
        })
    }

    #[must_use]
    pub fn fonts(&self) -> &[ResourceEntry] {
        &self.fonts
    }

    pub fn bind_font(&mut self, name: Vec<u8>, source: ByteStore, value: Object) -> bool {
        let Some(mut entry) = self.inline_entry(source, value) else {
            return false;
        };
        entry.name = name;
        self.fonts.retain(|font| font.name != entry.name);
        self.fonts.push(entry);
        true
    }

    #[must_use]
    pub fn color_space(&self, name: &[u8]) -> Option<&ResourceEntry> {
        self.color_spaces
            .iter()
            .find(|resource| resource.name == name)
    }

    #[must_use]
    pub fn color_spaces(&self) -> &[ResourceEntry] {
        &self.color_spaces
    }

    #[must_use]
    pub fn pattern(&self, name: &[u8]) -> Option<&ResourceEntry> {
        self.patterns.iter().find(|resource| resource.name == name)
    }

    #[must_use]
    pub fn patterns(&self) -> &[ResourceEntry] {
        &self.patterns
    }

    #[must_use]
    pub fn shading(&self, name: &[u8]) -> Option<&ResourceEntry> {
        self.shadings.iter().find(|resource| resource.name == name)
    }

    #[must_use]
    pub fn shadings(&self) -> &[ResourceEntry] {
        &self.shadings
    }

    #[must_use]
    pub fn property_list(&self, name: &[u8]) -> Option<&ResourceEntry> {
        self.properties
            .iter()
            .find(|resource| resource.name == name)
    }

    #[must_use]
    pub fn property_lists(&self) -> &[ResourceEntry] {
        &self.properties
    }
}

#[derive(Clone, Debug)]
pub struct ResourceEntry {
    name: Vec<u8>,
    reference: Option<Reference>,
    source: ByteStore,
    value: Object,
    form: Option<Arc<FormXObject>>,
    form_error: Option<PageContentErrorKind>,
    document: ByteStore,
    index: Arc<RevisionIndex>,
    security: Option<Arc<AuthenticatedSecurity>>,
    optional_content: Arc<OptionalContent>,
    limits: PageContentLimits,
}

impl ResourceEntry {
    #[must_use]
    pub fn name(&self) -> &[u8] {
        &self.name
    }

    #[must_use]
    pub const fn reference(&self) -> Option<Reference> {
        self.reference
    }

    #[must_use]
    pub const fn source(&self) -> &ByteStore {
        &self.source
    }

    #[must_use]
    pub const fn value(&self) -> &Object {
        &self.value
    }

    #[must_use]
    pub fn form(&self) -> Option<&FormXObject> {
        self.form.as_deref()
    }

    #[must_use]
    pub const fn form_unavailable(&self) -> Option<PageContentErrorKind> {
        self.form_error
    }

    #[must_use]
    pub fn is_same_object(&self, other: &Self) -> bool {
        let same_security = match (&self.security, &other.security) {
            (None, None) => true,
            (Some(mine), Some(theirs)) => Arc::ptr_eq(mine, theirs),
            _ => false,
        };
        self.reference.is_some()
            && self.reference == other.reference
            && same_security
            && self.limits == other.limits
            && Arc::ptr_eq(&self.index, &other.index)
            && self.document.is_same(&other.document)
            && self.source.is_same(&other.source)
            && self.value == other.value
    }

    pub fn simple_font(&self) -> Result<SimpleFont, FontError> {
        parse_simple_font(&self.source, &self.value, &self.font_resolver())
    }

    fn font_resolver(&self) -> impl Fn(Reference) -> Result<(ByteStore, Object), FontError> + '_ {
        move |reference| {
            let resolved = self
                .index
                .resolve_object(&self.document, reference, self.limits.resolve)
                .map_err(|_| FontError::IndirectEntryUnresolved)?;
            Ok((resolved.source().clone(), resolved.value().clone()))
        }
    }

    pub fn font(&self) -> Result<Font, ResourceFontError> {
        let ObjectKind::Dictionary(entries) = self.value.kind() else {
            return Err(ResourceFontError::Font(FontError::NotDictionary));
        };
        let subtype = entry(entries, &self.source, b"/Subtype")
            .ok_or(ResourceFontError::Font(FontError::MissingEntry))?;
        if !subtype.name_equals(&self.source, b"/Type0") {
            return parse_simple_font(&self.source, &self.value, &self.font_resolver())
                .map(Font::Simple)
                .map_err(ResourceFontError::Font);
        }
        let type0 = parse_type0_font(&self.source, &self.value, &self.font_resolver())
            .map_err(ResourceFontError::Font)?;
        let cmap = match type0.encoding {
            Type0Encoding::IdentityHorizontal => CMap::identity_horizontal(),
            Type0Encoding::IdentityVertical => {
                return Err(ResourceFontError::Font(
                    FontError::VerticalWritingUnsupported,
                ));
            }
            Type0Encoding::Predefined(name) => (*pdf_font::predefined::load(name.as_bytes())
                .map_err(ResourceFontError::CMap)?)
            .clone(),
            Type0Encoding::Stream(reference) => self.load_cmap(reference)?,
        };
        let descendant = match &type0.descendant {
            pdf_font::Descendant::Indirect(reference) => {
                let resolved = self
                    .index
                    .resolve_object(&self.document, *reference, self.limits.resolve)
                    .map_err(ResourceFontError::Resolve)?;
                parse_cid_font(resolved.source(), resolved.value(), &self.font_resolver())
            }
            pdf_font::Descendant::Direct(source, value) => {
                parse_cid_font(source, value, &self.font_resolver())
            }
        }
        .map_err(ResourceFontError::Font)?;
        Ok(Font::Composite(CompositeFont::new(
            cmap,
            descendant,
            type0.dictionary_span,
        )))
    }

    pub fn pattern_type(&self) -> Result<i64, PageContentError> {
        let (source, value) = match self.reference {
            Some(reference) => {
                let resolved = self
                    .index
                    .resolve_object(&self.document, reference, self.limits.resolve)
                    .map_err(|error| PageContentError::new(PageContentErrorKind::Resolve(error)))?;
                (resolved.source().clone(), resolved.value().clone())
            }
            None => (self.source.clone(), self.value.clone()),
        };
        let entries = dictionary(&value)
            .ok_or_else(|| PageContentError::new(PageContentErrorKind::PatternNotDictionary))?;
        let pattern_type = unique_pattern_entry(entries, &source, b"/PatternType")?
            .ok_or_else(|| PageContentError::new(PageContentErrorKind::PatternMissingType))?;
        let bytes = source
            .resolve(pattern_type.span())
            .map_err(|_| PageContentError::new(PageContentErrorKind::SourceSpanFailure))?;
        if !matches!(pattern_type.kind(), ObjectKind::Number(NumberKind::Integer)) {
            return Err(PageContentError::new(
                PageContentErrorKind::PatternWrongType,
            ));
        }
        std::str::from_utf8(bytes)
            .ok()
            .and_then(|text| text.parse::<i64>().ok())
            .ok_or_else(|| PageContentError::new(PageContentErrorKind::PatternWrongType))
    }

    pub fn shading_pattern(&self) -> Result<ShadingPattern, PageContentError> {
        let (source, value) = match self.reference {
            Some(reference) => {
                let resolved = self
                    .index
                    .resolve_object(&self.document, reference, self.limits.resolve)
                    .map_err(|error| PageContentError::new(PageContentErrorKind::Resolve(error)))?;
                (resolved.source().clone(), resolved.value().clone())
            }
            None => (self.source.clone(), self.value.clone()),
        };
        let entries = dictionary(&value)
            .ok_or_else(|| PageContentError::new(PageContentErrorKind::PatternNotDictionary))?;
        if self.pattern_type()? != 2 {
            return Err(PageContentError::new(
                PageContentErrorKind::PatternWrongType,
            ));
        }
        let shading_value = unique_pattern_entry(entries, &source, b"/Shading")?
            .ok_or_else(|| PageContentError::new(PageContentErrorKind::PatternMissingShading))?;
        let (shading_reference, shading_source, shading_object) = match shading_value.kind() {
            ObjectKind::Reference(reference) => {
                let resolved = self
                    .index
                    .resolve_object(&self.document, *reference, self.limits.resolve)
                    .map_err(|error| PageContentError::new(PageContentErrorKind::Resolve(error)))?;
                (
                    Some(*reference),
                    resolved.source().clone(),
                    resolved.value().clone(),
                )
            }
            _ => (None, source.clone(), shading_value.clone()),
        };
        Ok(ShadingPattern {
            reference: self.reference,
            source,
            dictionary: value,
            shading: ResourceEntry {
                name: self.name.clone(),
                reference: shading_reference,
                source: shading_source,
                value: shading_object,
                form: None,
                form_error: None,
                document: self.document.clone(),
                index: Arc::clone(&self.index),
                security: self.security.clone(),
                optional_content: Arc::clone(&self.optional_content),
                limits: self.limits,
            },
        })
    }

    pub fn tiling_pattern(&self) -> Result<TilingPattern, PageContentError> {
        let reference = self
            .reference
            .ok_or_else(|| PageContentError::new(PageContentErrorKind::PatternMustBeIndirect))?;
        load_tiling_pattern(
            &self.document,
            &self.index,
            self.security.as_ref(),
            reference,
            Arc::clone(&self.optional_content),
            self.limits,
        )
    }

    pub fn type3_font(&self) -> Result<Option<Type3Font>, PageContentError> {
        let Some(entries) = dictionary(&self.value) else {
            return Ok(None);
        };
        let Some(subtype) = entry(entries, &self.source, b"/Subtype") else {
            return Ok(None);
        };
        if !subtype.name_equals(&self.source, b"/Type3") {
            return Ok(None);
        }
        let matrix_value = entry(entries, &self.source, b"/FontMatrix")
            .ok_or_else(|| PageContentError::new(PageContentErrorKind::Type3MissingFontMatrix))?;
        let (matrix_source, matrix_value) = self.follow(matrix_value)?;
        let font_matrix = six_numbers(&matrix_source, &matrix_value)
            .ok_or_else(|| PageContentError::new(PageContentErrorKind::Type3InvalidFontMatrix))?;
        let mut procedures = BTreeMap::new();
        if let Some(procs) = entry(entries, &self.source, b"/CharProcs") {
            let (procs_source, procs) = self.follow(procs)?;
            let procs_entries = dictionary(&procs).ok_or_else(|| {
                PageContentError::new(PageContentErrorKind::Type3CharProcsNotDictionary)
            })?;
            if procs_entries.len() > self.limits.max_type3_procedures {
                return Err(PageContentError::new(
                    PageContentErrorKind::Type3ProcedureLimit,
                ));
            }
            for procedure in procs_entries {
                let key = procedure
                    .decoded_key(&procs_source)
                    .map_err(|_| PageContentError::new(PageContentErrorKind::SourceSpanFailure))?;
                let name = key.get(1..).unwrap_or_default().to_vec();
                let ObjectKind::Reference(reference) = procedure.value().kind() else {
                    return Err(PageContentError::new(
                        PageContentErrorKind::Type3ProcedureNotStream,
                    ));
                };
                let bytes = self.stream_bytes(*reference)?;
                let derivation = 0x5433_0000_0000_0000_u64
                    | (u64::from(reference.object_number()) << 16)
                    | u64::from(reference.generation());
                procedures.insert(
                    name,
                    Type3Procedure {
                        reference: *reference,
                        bytes: ByteStore::new(
                            SourceId::derived(self.document.id(), derivation),
                            Arc::<[u8]>::from(bytes.to_vec()),
                        ),
                    },
                );
            }
        }
        let resources = match entry(entries, &self.source, b"/Resources") {
            Some(value) => {
                let dictionary = resolve_resource_dictionary(
                    &self.document,
                    &self.index,
                    &self.source,
                    value,
                    self.limits,
                )?;
                let mut state = ResourceLoadState {
                    forms: HashMap::new(),
                    visiting: Vec::new(),
                    loaded_forms: 0,
                    security: self.security.clone(),
                    optional_content: Arc::clone(&self.optional_content),
                };
                Some(load_resources(
                    &self.document,
                    &self.index,
                    &dictionary,
                    self.limits,
                    &mut state,
                )?)
            }
            None => None,
        };
        Ok(Some(Type3Font {
            reference: self.reference,
            font_matrix,
            font_matrix_span: matrix_value.span(),
            resources,
            procedures,
        }))
    }

    pub fn form_reference(
        &self,
        reference: Reference,
    ) -> Result<Arc<FormXObject>, PageContentError> {
        let mut state = ResourceLoadState {
            forms: HashMap::new(),
            visiting: Vec::new(),
            loaded_forms: 0,
            security: self.security.clone(),
            optional_content: Arc::clone(&self.optional_content),
        };
        load_form_xobject(
            &self.document,
            &self.index,
            self.security.as_deref(),
            reference,
            self.limits,
            &mut state,
        )
    }

    pub fn icc_profile(
        &self,
        reference: Reference,
        max_decoded_bytes: usize,
    ) -> Result<IccProfileStream, PageContentError> {
        load_icc_profile(
            &self.document,
            &self.index,
            self.security.as_deref(),
            reference,
            self.limits,
            max_decoded_bytes,
        )
    }

    pub fn indexed_lookup(
        &self,
        reference: Reference,
        max_decoded_bytes: usize,
    ) -> Result<IndexedLookupStream, PageContentError> {
        load_indexed_lookup(
            &self.document,
            &self.index,
            self.security.as_deref(),
            reference,
            self.limits,
            max_decoded_bytes,
        )
    }

    pub fn function(
        &self,
        reference: Reference,
        max_decoded_bytes: usize,
    ) -> Result<FunctionObject, PageContentError> {
        load_function(
            &self.document,
            &self.index,
            self.security.as_deref(),
            reference,
            self.limits,
            max_decoded_bytes,
        )
    }

    pub fn image(
        &self,
        reference: Reference,
        max_decoded_bytes: usize,
    ) -> Result<ImageXObject, PageContentError> {
        load_image(
            &self.document,
            &self.index,
            self.security.as_deref(),
            reference,
            self.limits,
            max_decoded_bytes,
        )
    }

    pub fn resolve_object(
        &self,
        reference: Reference,
    ) -> Result<(ByteStore, Object), PageContentError> {
        let resolved = self
            .index
            .resolve_object(&self.document, reference, self.limits.resolve)
            .map_err(|error| PageContentError::new(PageContentErrorKind::Resolve(error)))?;
        Ok((resolved.source().clone(), resolved.value().clone()))
    }

    pub fn font_request(&self) -> Result<Option<FontRequest>, PageContentError> {
        self.request(false)
    }

    pub fn face_request(&self) -> Result<Option<FontRequest>, PageContentError> {
        self.request(true)
    }

    fn request(&self, type3: bool) -> Result<Option<FontRequest>, PageContentError> {
        let ObjectKind::Dictionary(entries) = self.value.kind() else {
            return Ok(None);
        };
        let subtype = entry(entries, &self.source, b"/Subtype")
            .map(|value| Self::name_bytes(&self.source, value))
            .unwrap_or_default();
        if subtype == b"/Type3" && !type3 {
            return Ok(None);
        }
        let mut base_font = entry(entries, &self.source, b"/BaseFont")
            .map(|value| Self::name_bytes(&self.source, value))
            .unwrap_or_default();
        let mut cid_subtype = None;
        let mut registry = None;
        let mut ordering = None;
        let encoding = entry(entries, &self.source, b"/Encoding")
            .filter(|value| matches!(value.kind(), ObjectKind::Name))
            .map(|value| {
                String::from_utf8_lossy(&Self::name_bytes(&self.source, value)).into_owned()
            });
        let (holder_source, holder) = match entry(entries, &self.source, b"/DescendantFonts") {
            Some(value) => {
                let (source, value) = self.follow(value)?;
                let ObjectKind::Array(items) = value.kind() else {
                    return Ok(None);
                };
                let Some(first) = items.first() else {
                    return Ok(None);
                };
                let (source, holder) = self.follow_in(&source, first)?;
                if let ObjectKind::Dictionary(holder_entries) = holder.kind() {
                    cid_subtype = entry(holder_entries, &source, b"/Subtype")
                        .map(|value| Self::name_bytes(&source, value));
                    if let Some(info) = entry(holder_entries, &source, b"/CIDSystemInfo") {
                        let (info_source, info) = self.follow_in(&source, info)?;
                        if let ObjectKind::Dictionary(info_entries) = info.kind() {
                            registry = entry(info_entries, &info_source, b"/Registry")
                                .and_then(|value| Self::string_text(&info_source, value));
                            ordering = entry(info_entries, &info_source, b"/Ordering")
                                .and_then(|value| Self::string_text(&info_source, value));
                        }
                    }
                }
                (source, holder)
            }
            None => (self.source.clone(), self.value.clone()),
        };
        let descriptor = self.font_descriptor(&holder_source, &holder)?;
        let Descriptor {
            flags,
            italic_angle,
            ascent,
            descent,
            declared_weight,
            present: descriptor_present,
            embedded_key,
            font_name,
            font_family,
        } = descriptor;
        if base_font.is_empty()
            && let Some(name) = font_name
        {
            base_font = name;
        }
        let (mut family, style) = declared_style(&base_font, declared_weight, flags, italic_angle);
        if subtype == b"/Type3"
            && let Some(named) = font_family.filter(|named| !named.trim().is_empty())
        {
            family = named;
        }
        let standard_face = Standard14::from_base_font(&base_font);
        let program = match embedded_key {
            None if descriptor_present => ProgramEvidence::NotEmbedded,
            None => ProgramEvidence::NoDescriptor,
            Some(key) => ProgramEvidence::Embedded { key: key.to_vec() },
        };
        Ok(Some(FontRequest {
            base_font,
            family,
            style,
            subtype,
            cid_subtype,
            registry,
            ordering,
            encoding,
            flags,
            italic_angle,
            ascent,
            descent,
            standard_face,
            program,
        }))
    }

    fn font_descriptor(
        &self,
        holder_source: &ByteStore,
        holder: &Object,
    ) -> Result<Descriptor, PageContentError> {
        let mut found = Descriptor::default();
        let ObjectKind::Dictionary(holder_entries) = holder.kind() else {
            return Ok(found);
        };
        let Some(descriptor) = entry(holder_entries, holder_source, b"/FontDescriptor") else {
            return Ok(found);
        };
        let (source, descriptor) = self.follow_in(holder_source, descriptor)?;
        let ObjectKind::Dictionary(entries) = descriptor.kind() else {
            return Ok(found);
        };
        found.present = true;
        if let Some(value) = entry(entries, &source, b"/Flags")
            && let Some(number) = Self::integer(&source, value)
        {
            found.flags = FontFlags(u32::try_from(number).unwrap_or(0));
        }
        found.italic_angle =
            entry(entries, &source, b"/ItalicAngle").and_then(|value| Self::number(&source, value));
        found.declared_weight =
            entry(entries, &source, b"/FontWeight").and_then(|value| Self::integer(&source, value));
        found.ascent =
            entry(entries, &source, b"/Ascent").and_then(|value| Self::number(&source, value));
        found.font_name = entry(entries, &source, b"/FontName")
            .filter(|value| matches!(value.kind(), ObjectKind::Name))
            .map(|value| Self::name_bytes(&source, value));
        found.font_family = entry(entries, &source, b"/FontFamily")
            .and_then(|value| pdf_syntax::decode_string(&source, value, FAMILY_NAME_BYTES).ok())
            .map(|bytes| {
                bytes
                    .iter()
                    .filter(|byte| byte.is_ascii_graphic() || **byte == b' ')
                    .map(|byte| char::from(*byte))
                    .collect::<String>()
            });
        found.descent =
            entry(entries, &source, b"/Descent").and_then(|value| Self::number(&source, value));
        for key in [&b"/FontFile"[..], b"/FontFile2", b"/FontFile3"] {
            if entry(entries, &source, key).is_some() {
                found.embedded_key = Some(key);
                break;
            }
        }
        Ok(found)
    }

    fn name_bytes(source: &ByteStore, object: &Object) -> Vec<u8> {
        match pdf_syntax::decode_name(source, object) {
            Ok(name) => name,
            Err(_) => source
                .resolve(object.span())
                .map(<[u8]>::to_vec)
                .unwrap_or_default(),
        }
    }

    fn string_text(source: &ByteStore, object: &Object) -> Option<String> {
        if !matches!(
            object.kind(),
            ObjectKind::LiteralString | ObjectKind::HexString
        ) {
            return None;
        }
        let bytes = source.resolve(object.span()).ok()?;
        let inner = bytes
            .strip_prefix(b"(")
            .and_then(|rest| rest.strip_suffix(b")"))
            .or_else(|| {
                bytes
                    .strip_prefix(b"<")
                    .and_then(|rest| rest.strip_suffix(b">"))
            })
            .unwrap_or(bytes);
        Some(
            inner
                .iter()
                .filter(|byte| byte.is_ascii_graphic())
                .map(|byte| *byte as char)
                .collect(),
        )
    }

    fn integer(source: &ByteStore, object: &Object) -> Option<i64> {
        if !matches!(object.kind(), ObjectKind::Number(NumberKind::Integer)) {
            return None;
        }
        let bytes = source.resolve(object.span()).ok()?;
        std::str::from_utf8(bytes).ok()?.parse().ok()
    }

    fn number(source: &ByteStore, object: &Object) -> Option<f64> {
        if !matches!(object.kind(), ObjectKind::Number(_)) {
            return None;
        }
        let bytes = source.resolve(object.span()).ok()?;
        std::str::from_utf8(bytes).ok()?.parse().ok()
    }

    pub fn glyph_program(&self) -> Result<Option<Arc<GlyphProgram>>, PageContentError> {
        let ObjectKind::Dictionary(entries) = self.value.kind() else {
            return Ok(None);
        };
        let (source, holder) = match entry(entries, &self.source, b"/DescendantFonts") {
            Some(value) => {
                let (source, value) = self.follow(value)?;
                let ObjectKind::Array(items) = value.kind() else {
                    return Ok(None);
                };
                let Some(first) = items.first() else {
                    return Ok(None);
                };
                self.follow_in(&source, first)?
            }
            None => (self.source.clone(), self.value.clone()),
        };
        let ObjectKind::Dictionary(holder_entries) = holder.kind() else {
            return Ok(None);
        };
        let Some(descriptor) = entry(holder_entries, &source, b"/FontDescriptor") else {
            return Ok(None);
        };
        let (descriptor_source, descriptor) = self.follow_in(&source, descriptor)?;
        let ObjectKind::Dictionary(descriptor_entries) = descriptor.kind() else {
            return Ok(None);
        };
        for (key, cff) in [
            (&b"/FontFile2"[..], false),
            (&b"/FontFile3"[..], true),
            (&b"/FontFile"[..], false),
        ] {
            let Some(program) = entry(descriptor_entries, &descriptor_source, key) else {
                continue;
            };
            let ObjectKind::Reference(reference) = program.kind() else {
                continue;
            };
            let bytes = self.stream_bytes(*reference)?;
            let _ = cff;
            let parsed = GlyphProgram::parse(bytes.to_vec())
                .map_err(|error| PageContentError::new(PageContentErrorKind::FontProgram(error)))?;
            return Ok(Some(Arc::new(parsed)));
        }
        Ok(None)
    }

    pub fn to_unicode(&self) -> Result<Arc<ToUnicode>, PageContentError> {
        let ObjectKind::Dictionary(entries) = self.value.kind() else {
            return Ok(Arc::new(ToUnicode::default()));
        };
        let mut map = match entry(entries, &self.source, b"/ToUnicode") {
            Some(object) => match object.kind() {
                ObjectKind::Reference(reference) => {
                    let bytes = self.stream_bytes(*reference)?;
                    let derivation = 0x5455_0000_0000_0000_u64
                        | (u64::from(reference.object_number()) << 16)
                        | u64::from(reference.generation());
                    let source =
                        ByteStore::new(SourceId::derived(self.document.id(), derivation), bytes);
                    ToUnicode::read_or_refused(&source, CMapLimits::default())
                }
                _ => ToUnicode::default(),
            },
            None => ToUnicode::default(),
        };
        let base_font = entry(entries, &self.source, b"/BaseFont")
            .map(|value| Self::name_bytes(&self.source, value))
            .unwrap_or_default();
        if ToUnicode::names_the_symbol_family(&base_font) {
            map.read_symbol_private_use();
        }
        if ToUnicode::names_the_mt_extra_family(&base_font) {
            map.read_mt_extra_private_use();
        }
        if entry(entries, &self.source, b"/DescendantFonts").is_none()
            && let Ok(font) = self.simple_font()
        {
            let program = self.glyph_program().ok().flatten();
            let names: Vec<(u8, Vec<u8>)> = (0..=u8::MAX)
                .filter_map(|code| {
                    let declared = font.glyph_name(code);
                    let drawn = program.as_deref().and_then(|program| {
                        let glyph = declared
                            .and_then(|name| program.glyph_for_name(name))
                            .or_else(|| program.glyph_for_code(code))?;
                        program.glyph_name(glyph)
                    });
                    if let Some(character) = font.standard_duplicate(code) {
                        let name: &[u8] = if character == '\u{ad}' {
                            b"sfthyphen"
                        } else {
                            b"nbspace"
                        };
                        return Some((code, name.to_vec()));
                    }
                    drawn.or(declared).map(|name| (code, name.to_vec()))
                })
                .collect();
            map.add_glyph_names(names.iter().map(|(code, name)| (*code, name.as_slice())));
            let sfnt = program.as_deref().and_then(|program| match program {
                GlyphProgram::TrueType(font) => Some(font),
                GlyphProgram::Cff { sfnt, .. } => sfnt.as_deref(),
                GlyphProgram::Type1 { .. } => None,
            });
            if let Some(sfnt) = sfnt {
                let by_glyph = sfnt.characters();
                let characters: Vec<(u8, char)> = (0..=u8::MAX)
                    .filter_map(|code| sfnt.glyph_for_code(code).map(|(glyph, _)| (code, glyph)))
                    .flat_map(|(code, glyph)| {
                        by_glyph
                            .iter()
                            .filter(move |(found, _)| *found == glyph)
                            .map(move |(_, character)| (code, *character))
                    })
                    .collect();
                map.add_characters(characters);
            }
        }
        Ok(Arc::new(map))
    }

    fn follow(&self, value: &Object) -> Result<(ByteStore, Object), PageContentError> {
        self.follow_in(&self.source, value)
    }

    fn follow_in(
        &self,
        source: &ByteStore,
        value: &Object,
    ) -> Result<(ByteStore, Object), PageContentError> {
        match value.kind() {
            ObjectKind::Reference(reference) => self.resolve_object(*reference),
            _ => Ok((source.clone(), value.clone())),
        }
    }

    fn stream_bytes(&self, reference: Reference) -> Result<Arc<[u8]>, PageContentError> {
        let resolved = self
            .index
            .resolve_object(&self.document, reference, self.limits.resolve)
            .map_err(|error| PageContentError::new(PageContentErrorKind::Resolve(error)))?;
        let ObjectKind::Dictionary(entries) = resolved.value().kind() else {
            return Err(PageContentError::new(
                PageContentErrorKind::StreamNotDictionary,
            ));
        };
        let stream = resolved
            .stream()
            .ok_or_else(|| PageContentError::new(PageContentErrorKind::ContentsNotStream))?;
        let encoded = stream_plaintext(self.security.as_deref(), &resolved, reference, stream)?;
        decode_stream_bytes(
            resolved.source(),
            entries,
            &encoded,
            stream.data_span().start(),
            self.limits.max_decoded_stream_bytes,
        )
        .map(Arc::<[u8]>::from)
        .map_err(|error| PageContentError::new(PageContentErrorKind::Decode(error)))
    }

    fn load_cmap(&self, reference: Reference) -> Result<CMap, ResourceFontError> {
        let resolved = self
            .index
            .resolve_object(&self.document, reference, self.limits.resolve)
            .map_err(ResourceFontError::Resolve)?;
        let ObjectKind::Dictionary(entries) = resolved.value().kind() else {
            return Err(ResourceFontError::CMapNotStream);
        };
        let stream = resolved.stream().ok_or(ResourceFontError::CMapNotStream)?;
        let encoded = resolved
            .source()
            .resolve(stream.data_span())
            .map_err(|_| ResourceFontError::SourceSpanFailure)?;
        let decoded = decode_stream_bytes(
            resolved.source(),
            entries,
            encoded,
            stream.data_span().start(),
            self.limits.max_decoded_stream_bytes,
        )
        .map_err(ResourceFontError::Decode)?;
        let derivation = 0x434d_0000_0000_0000_u64
            | (u64::from(reference.object_number()) << 16)
            | u64::from(reference.generation());
        let source = ByteStore::new(
            SourceId::derived(self.document.id(), derivation),
            Arc::<[u8]>::from(decoded),
        );
        parse_cmap(&source, CMapLimits::default()).map_err(ResourceFontError::CMap)
    }
}

#[derive(Clone, Debug)]
pub struct IccProfileStream {
    pub reference: Reference,
    pub source: ByteStore,
    pub dictionary: Object,
    pub encoded_data_span: pdf_bytes::SourceSpan,
    pub bytes: Arc<[u8]>,
}

#[derive(Clone, Debug)]
pub struct IndexedLookupStream {
    pub reference: Reference,
    pub source: ByteStore,
    pub dictionary: Object,
    pub encoded_data_span: pdf_bytes::SourceSpan,
    pub bytes: Arc<[u8]>,
}

#[derive(Clone, Debug)]
pub struct ImageXObject {
    pub reference: Reference,
    pub source: ByteStore,
    pub dictionary: Object,
    pub encoded_data_span: pdf_bytes::SourceSpan,
    pub bytes: Arc<[u8]>,
    pub codec: Option<ImageCodec>,
    pub codec_span: Option<pdf_bytes::SourceSpan>,
    pub codec_parameters: Option<Object>,
    pub repairs: Vec<StreamRepair>,
}

#[derive(Clone, Debug)]
pub struct FunctionObject {
    pub reference: Reference,
    pub source: ByteStore,
    pub dictionary: Object,
    pub data: Option<FunctionData>,
}

#[derive(Clone, Debug)]
pub struct FunctionData {
    pub encoded_data_span: pdf_bytes::SourceSpan,
    pub bytes: Arc<[u8]>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ResourceFontError {
    Font(FontError),
    Resolve(ResolveError),
    Decode(StreamDecodeError),
    CMap(CMapError),
    CMapNotStream,
    SourceSpanFailure,
}

impl fmt::Display for ResourceFontError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Font(error) => error.fmt(formatter),
            Self::Resolve(error) => error.fmt(formatter),
            Self::Decode(error) => error.fmt(formatter),
            Self::CMap(error) => error.fmt(formatter),
            Self::CMapNotStream => formatter.write_str("Type0 CMap is not an indirect stream"),
            Self::SourceSpanFailure => formatter.write_str("CMap stream span cannot be resolved"),
        }
    }
}

impl std::error::Error for ResourceFontError {}

#[derive(Clone, Debug)]
pub struct FormXObject {
    pub reference: Reference,
    pub source: ByteStore,
    pub dictionary: Object,
    pub bytes: ByteStore,
    pub resources: Option<PageResources>,
    document: ByteStore,
    index: Arc<RevisionIndex>,
    security: Option<Arc<AuthenticatedSecurity>>,
    limits: PageContentLimits,
}

impl FormXObject {
    pub fn resolve_object(
        &self,
        reference: Reference,
    ) -> Result<(ByteStore, Object), PageContentError> {
        let resolved = self
            .index
            .resolve_object(&self.document, reference, self.limits.resolve)
            .map_err(|error| PageContentError::new(PageContentErrorKind::Resolve(error)))?;
        Ok((resolved.source().clone(), resolved.value().clone()))
    }

    pub fn icc_profile(
        &self,
        reference: Reference,
        max_decoded_bytes: usize,
    ) -> Result<IccProfileStream, PageContentError> {
        load_icc_profile(
            &self.document,
            &self.index,
            self.security.as_deref(),
            reference,
            self.limits,
            max_decoded_bytes,
        )
    }

    pub fn indexed_lookup(
        &self,
        reference: Reference,
        max_decoded_bytes: usize,
    ) -> Result<IndexedLookupStream, PageContentError> {
        load_indexed_lookup(
            &self.document,
            &self.index,
            self.security.as_deref(),
            reference,
            self.limits,
            max_decoded_bytes,
        )
    }

    pub fn function(
        &self,
        reference: Reference,
        max_decoded_bytes: usize,
    ) -> Result<FunctionObject, PageContentError> {
        load_function(
            &self.document,
            &self.index,
            self.security.as_deref(),
            reference,
            self.limits,
            max_decoded_bytes,
        )
    }
}

#[derive(Clone, Debug)]
pub struct ShadingPattern {
    pub reference: Option<Reference>,
    pub source: ByteStore,
    pub dictionary: Object,
    pub shading: ResourceEntry,
}

#[derive(Clone, Debug)]
pub struct TilingPattern {
    pub reference: Reference,
    pub source: ByteStore,
    pub dictionary: Object,
    pub bytes: ByteStore,
    pub resources: PageResources,
}

#[derive(Clone, Debug)]
pub struct Type3Font {
    pub reference: Option<Reference>,
    pub font_matrix: [f64; 6],
    pub font_matrix_span: pdf_bytes::SourceSpan,
    pub resources: Option<PageResources>,
    procedures: BTreeMap<Vec<u8>, Type3Procedure>,
}

#[derive(Clone, Debug)]
pub struct Type3Procedure {
    pub reference: Reference,
    pub bytes: ByteStore,
}

impl Type3Font {
    #[must_use]
    pub fn procedure(&self, name: &[u8]) -> Option<&Type3Procedure> {
        self.procedures.get(name)
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.procedures.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.procedures.is_empty()
    }

    #[must_use]
    pub const fn advance_scale(&self) -> f64 {
        self.font_matrix[0]
    }
}

#[derive(Clone, Debug)]
struct ResourceDictionary {
    source: ByteStore,
    value: Object,
}

#[derive(Clone, Debug)]
struct FoundPage {
    reference: Reference,
    resources: Option<ResourceDictionary>,
    geometry: InheritedGeometry,
}

#[derive(Clone, Debug, Default)]
struct InheritedGeometry {
    media_box: Option<(ByteStore, Object)>,
    crop_box: Option<(ByteStore, Object)>,
    rotate: Option<(ByteStore, Object)>,
}

struct ResourceLoadState {
    forms: HashMap<Reference, Arc<FormXObject>>,
    visiting: Vec<Reference>,
    loaded_forms: usize,
    security: Option<Arc<AuthenticatedSecurity>>,
    optional_content: Arc<OptionalContent>,
}

#[derive(Clone, Debug, Default)]
pub struct OptionalContent {
    known: HashSet<Reference>,
    hidden: HashSet<Reference>,
    medium: Medium,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Medium {
    #[default]
    Screen,
    Print,
}

impl Medium {
    const fn usage_keys(self) -> (&'static [u8], &'static [u8]) {
        match self {
            Self::Screen => (b"/View", b"/ViewState"),
            Self::Print => (b"/Print", b"/PrintState"),
        }
    }
}

impl OptionalContent {
    #[must_use]
    pub fn group_hidden(&self, group: Reference) -> bool {
        self.hidden.contains(&group)
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.known.is_empty()
    }

    #[must_use]
    pub const fn medium(&self) -> Medium {
        self.medium
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ViewState {
    On,
    Off,
}

fn usage_state(
    document: &ByteStore,
    index: &RevisionIndex,
    source: &ByteStore,
    usage: &Object,
    (medium, limits): (Medium, PageContentLimits),
) -> Result<Option<ViewState>, PageContentError> {
    let (key, state_key) = medium.usage_keys();
    let (usage_source, usage) = resolve_direct(document, index, source, usage, limits)?;
    let Some(entries) = dictionary(&usage) else {
        return Ok(None);
    };
    let Some(view) = entry(entries, &usage_source, key) else {
        return Ok(None);
    };
    let (view_source, view) = resolve_direct(document, index, &usage_source, view, limits)?;
    let Some(view_entries) = dictionary(&view) else {
        return Ok(None);
    };
    let Some(state) = entry(view_entries, &view_source, state_key) else {
        return Ok(None);
    };
    Ok(Some(if state.name_equals(&view_source, b"/OFF") {
        ViewState::Off
    } else {
        ViewState::On
    }))
}

fn intended_for_view(
    document: &ByteStore,
    index: &RevisionIndex,
    source: &ByteStore,
    entries: &[pdf_syntax::DictionaryEntry],
    limits: PageContentLimits,
) -> Result<bool, PageContentError> {
    let Some(intent) = entry(entries, source, b"/Intent") else {
        return Ok(true);
    };
    let (intent_source, intent) = resolve_direct(document, index, source, intent, limits)?;
    let matches = |value: &Object| {
        value.name_equals(&intent_source, b"/View") || value.name_equals(&intent_source, b"/All")
    };
    Ok(match intent.kind() {
        ObjectKind::Array(items) => items.iter().any(matches),
        _ => matches(&intent),
    })
}

fn load_icc_profile(
    document: &ByteStore,
    index: &RevisionIndex,
    security: Option<&AuthenticatedSecurity>,
    reference: Reference,
    limits: PageContentLimits,
    max_decoded_bytes: usize,
) -> Result<IccProfileStream, PageContentError> {
    let resolved = index
        .resolve_object(document, reference, limits.resolve)
        .map_err(|error| PageContentError::new(PageContentErrorKind::Resolve(error)))?;
    if !matches!(resolved.value().kind(), ObjectKind::Dictionary(_)) {
        return Err(PageContentError::new(
            PageContentErrorKind::IccProfileNotDictionary,
        ));
    }
    let stream = resolved
        .stream()
        .ok_or_else(|| PageContentError::new(PageContentErrorKind::IccProfileMissingStream))?;
    let encoded = stream_plaintext(security, &resolved, reference, stream)?;
    let decoded = decode_stream_bytes(
        resolved.source(),
        match resolved.value().kind() {
            ObjectKind::Dictionary(entries) => entries,
            _ => unreachable!("dictionary checked above"),
        },
        &encoded,
        stream.data_span().start(),
        max_decoded_bytes.min(limits.max_decoded_stream_bytes),
    )
    .map_err(|error| PageContentError::new(PageContentErrorKind::Decode(error)))?;
    Ok(IccProfileStream {
        reference,
        source: resolved.source().clone(),
        dictionary: resolved.value().clone(),
        encoded_data_span: stream.data_span(),
        bytes: Arc::<[u8]>::from(decoded),
    })
}

fn authenticate_document(
    source: &ByteStore,
    chain: &pdf_syntax::RevisionChain,
    index: &RevisionIndex,
    trailer_entries: &[pdf_syntax::DictionaryEntry],
    limits: PageContentLimits,
    password: &[u8],
) -> Result<Option<Arc<AuthenticatedSecurity>>, PageContentError> {
    if entry(trailer_entries, source, b"/Encrypt").is_none() {
        return Ok(None);
    }
    authenticate_standard_password(source, chain, index, password, limits.resolve)
        .map(|session| Some(Arc::new(session)))
        .map_err(|error| PageContentError::new(PageContentErrorKind::Authenticate(error.kind())))
}

#[derive(Debug)]
struct ObjectStreamDecryptor(Arc<AuthenticatedSecurity>);

impl pdf_syntax::StreamDecryptor for ObjectStreamDecryptor {
    fn decrypt_stream(
        &self,
        reference: Reference,
        encrypted: &[u8],
    ) -> Result<Vec<u8>, DecryptionRefused> {
        self.0
            .decrypt_stream(reference, encrypted)
            .map_err(|error| {
                DecryptionRefused::new(match error.kind() {
                    SecurityErrorKind::InvalidCipherKey => "encryption key has an invalid length",
                    SecurityErrorKind::MalformedCiphertext => {
                        "encrypted object data or AES padding is malformed"
                    }
                    _ => "the security handler produced no plaintext",
                })
            })
    }
}

fn stream_plaintext<'a>(
    security: Option<&AuthenticatedSecurity>,
    resolved: &'a pdf_syntax::ResolvedObject,
    reference: Reference,
    stream: &pdf_syntax::StreamObject,
) -> Result<Cow<'a, [u8]>, PageContentError> {
    let encoded = resolved
        .source()
        .resolve(stream.data_span())
        .map_err(|_| PageContentError::new(PageContentErrorKind::SourceSpanFailure))?;
    let Some(security) = security else {
        return Ok(Cow::Borrowed(encoded));
    };
    if resolved.is_compressed() {
        return Ok(Cow::Borrowed(encoded));
    }
    security
        .decrypt_stream(reference, encoded)
        .map(Cow::Owned)
        .map_err(|error| PageContentError::new(PageContentErrorKind::Decrypt(error.kind())))
}

fn load_image(
    document: &ByteStore,
    index: &RevisionIndex,
    security: Option<&AuthenticatedSecurity>,
    reference: Reference,
    limits: PageContentLimits,
    max_decoded_bytes: usize,
) -> Result<ImageXObject, PageContentError> {
    let resolved = index
        .resolve_object(document, reference, limits.resolve)
        .map_err(|error| PageContentError::new(PageContentErrorKind::Resolve(error)))?;
    let ObjectKind::Dictionary(entries) = resolved.value().kind() else {
        return Err(PageContentError::new(
            PageContentErrorKind::ImageNotDictionary,
        ));
    };
    let stream = resolved
        .stream()
        .ok_or_else(|| PageContentError::new(PageContentErrorKind::ImageMissingStream))?;
    let encoded = stream_plaintext(security, &resolved, reference, stream)?;
    let decoded = decode_image_stream_bytes_recovering(
        resolved.source(),
        entries,
        &encoded,
        stream.data_span().start(),
        max_decoded_bytes.min(limits.max_decoded_stream_bytes),
    )
    .map_err(|error| PageContentError::new(PageContentErrorKind::Decode(error)))?;
    Ok(ImageXObject {
        reference,
        source: resolved.source().clone(),
        dictionary: resolved.value().clone(),
        encoded_data_span: stream.data_span(),
        bytes: Arc::<[u8]>::from(decoded.bytes),
        codec: decoded.codec,
        codec_span: decoded.codec_span,
        codec_parameters: decoded.codec_parameters,
        repairs: decoded.repairs,
    })
}

fn load_function(
    document: &ByteStore,
    index: &RevisionIndex,
    security: Option<&AuthenticatedSecurity>,
    reference: Reference,
    limits: PageContentLimits,
    max_decoded_bytes: usize,
) -> Result<FunctionObject, PageContentError> {
    let resolved = index
        .resolve_object(document, reference, limits.resolve)
        .map_err(|error| PageContentError::new(PageContentErrorKind::Resolve(error)))?;
    let ObjectKind::Dictionary(entries) = resolved.value().kind() else {
        return Err(PageContentError::new(
            PageContentErrorKind::FunctionNotDictionary,
        ));
    };
    let data = match resolved.stream() {
        Some(stream) => {
            let encoded = stream_plaintext(security, &resolved, reference, stream)?;
            let decoded = decode_stream_bytes(
                resolved.source(),
                entries,
                &encoded,
                stream.data_span().start(),
                max_decoded_bytes.min(limits.max_decoded_stream_bytes),
            )
            .map_err(|error| PageContentError::new(PageContentErrorKind::Decode(error)))?;
            Some(FunctionData {
                encoded_data_span: stream.data_span(),
                bytes: Arc::<[u8]>::from(decoded),
            })
        }
        None => None,
    };
    Ok(FunctionObject {
        reference,
        source: resolved.source().clone(),
        dictionary: resolved.value().clone(),
        data,
    })
}

fn load_indexed_lookup(
    document: &ByteStore,
    index: &RevisionIndex,
    security: Option<&AuthenticatedSecurity>,
    reference: Reference,
    limits: PageContentLimits,
    max_decoded_bytes: usize,
) -> Result<IndexedLookupStream, PageContentError> {
    let resolved = index
        .resolve_object(document, reference, limits.resolve)
        .map_err(|error| PageContentError::new(PageContentErrorKind::Resolve(error)))?;
    let ObjectKind::Dictionary(entries) = resolved.value().kind() else {
        return Err(PageContentError::new(
            PageContentErrorKind::IndexedLookupNotDictionary,
        ));
    };
    let stream = resolved
        .stream()
        .ok_or_else(|| PageContentError::new(PageContentErrorKind::IndexedLookupMissingStream))?;
    let encoded = stream_plaintext(security, &resolved, reference, stream)?;
    let decoded = decode_stream_bytes(
        resolved.source(),
        entries,
        &encoded,
        stream.data_span().start(),
        max_decoded_bytes.min(limits.max_decoded_stream_bytes),
    )
    .map_err(|error| PageContentError::new(PageContentErrorKind::Decode(error)))?;
    Ok(IndexedLookupStream {
        reference,
        source: resolved.source().clone(),
        dictionary: resolved.value().clone(),
        encoded_data_span: stream.data_span(),
        bytes: Arc::<[u8]>::from(decoded),
    })
}

pub fn load_page_program_strict(
    source: &ByteStore,
    page_index: usize,
    limits: PageContentLimits,
) -> Result<PageProgram, PageContentError> {
    load_page_program_with_password(source, page_index, limits, b"")
}

pub fn count_pages_strict(
    source: &ByteStore,
    limits: PageContentLimits,
) -> Result<usize, PageContentError> {
    count_pages_with_password(source, limits, b"")
}

pub fn count_pages_with_password(
    source: &ByteStore,
    limits: PageContentLimits,
    password: &[u8],
) -> Result<usize, PageContentError> {
    let tree = open_page_tree(source, limits, password)?;
    count_pages_from_tree(source, &tree, limits)
}

fn count_pages_from_tree(
    source: &ByteStore,
    tree: &PageTree,
    limits: PageContentLimits,
) -> Result<usize, PageContentError> {
    let mut pages = 0_usize;
    walk_page_tree(
        source,
        &tree.index,
        tree.root,
        limits,
        (),
        |_, ()| Ok(()),
        |_, ()| {
            pages += 1;
            Ok(ControlFlow::<()>::Continue(()))
        },
    )?;
    Ok(pages)
}

pub fn page_references_with_password(
    source: &ByteStore,
    limits: PageContentLimits,
    password: &[u8],
) -> Result<Vec<Reference>, PageContentError> {
    let tree = open_page_tree(source, limits, password)?;
    let mut pages = Vec::new();
    walk_page_tree(
        source,
        &tree.index,
        tree.root,
        limits,
        (),
        |_, ()| Ok(()),
        |page, ()| {
            pages.push(page);
            Ok(ControlFlow::<()>::Continue(()))
        },
    )?;
    Ok(pages)
}

pub fn page_geometries_with_password(
    source: &ByteStore,
    limits: PageContentLimits,
    password: &[u8],
) -> Result<Vec<PageGeometry>, PageContentError> {
    let tree = open_page_tree(source, limits, password)?;
    page_geometries_from_tree(source, &tree, limits)
}

fn page_geometries_from_tree(
    source: &ByteStore,
    tree: &PageTree,
    limits: PageContentLimits,
) -> Result<Vec<PageGeometry>, PageContentError> {
    let mut geometries = Vec::new();
    let mut failure = None;
    walk_page_tree(
        source,
        &tree.index,
        tree.root,
        limits,
        InheritedGeometry::default(),
        |node, inherited| Ok(inherited_geometry_of(node, inherited)),
        |_, geometry| match resolve_page_geometry(source, &tree.index, geometry, limits) {
            Ok(resolved) => {
                geometries.push(resolved);
                Ok(ControlFlow::Continue(()))
            }
            Err(error) => {
                failure = Some(error);
                Ok(ControlFlow::Break(()))
            }
        },
    )?;
    if let Some(error) = failure {
        return Err(error);
    }
    Ok(geometries)
}

fn inherited_geometry_of(
    node: &ResolvedObject,
    inherited: &InheritedGeometry,
) -> InheritedGeometry {
    let mut geometry = inherited.clone();
    let Some(entries) = dictionary(node.value()) else {
        return geometry;
    };
    for (key, slot) in [
        (&b"/MediaBox"[..], &mut geometry.media_box),
        (&b"/CropBox"[..], &mut geometry.crop_box),
        (&b"/Rotate"[..], &mut geometry.rotate),
    ] {
        if let Some(value) = entry(entries, node.source(), key) {
            *slot = Some((node.source().clone(), value.clone()));
        }
    }
    geometry
}

struct PageTree {
    index: Arc<RevisionIndex>,
    root: Reference,
    catalog: Reference,
    security: Option<Arc<AuthenticatedSecurity>>,
}

impl PageTree {
    fn tolerating_damage(&self) -> Self {
        Self {
            index: Arc::new(
                (*self.index)
                    .clone()
                    .tolerating_damage()
                    .resolving_undefined_as_null(),
            ),
            root: self.root,
            catalog: self.catalog,
            security: self.security.clone(),
        }
    }
}

fn open_page_tree(
    source: &ByteStore,
    limits: PageContentLimits,
    password: &[u8],
) -> Result<PageTree, PageContentError> {
    parse_header_strict(source)
        .map_err(|error| PageContentError::new(PageContentErrorKind::Header(error)))?;
    let chain = parse_revision_chain_strict(source, limits.xref)
        .map_err(|error| PageContentError::new(PageContentErrorKind::Revisions(error)))?;
    let index = RevisionIndex::from_chain(&chain)
        .map_err(|error| PageContentError::new(PageContentErrorKind::Resolve(error)))?;
    let trailer = chain
        .revisions()
        .first()
        .ok_or_else(|| PageContentError::new(PageContentErrorKind::MissingRevision))?
        .trailer();
    let trailer_entries = dictionary(trailer)
        .ok_or_else(|| PageContentError::new(PageContentErrorKind::TrailerNotDictionary))?;
    let security =
        authenticate_document(source, &chain, &index, trailer_entries, limits, password)?;
    let index = Arc::new(match security.as_ref() {
        Some(security) => {
            index.with_stream_decryptor(Arc::new(ObjectStreamDecryptor(Arc::clone(security))))
        }
        None => index,
    });
    let root = reference_entry(trailer_entries, source, b"/Root")
        .ok_or_else(|| PageContentError::new(PageContentErrorKind::MissingRoot))?;
    let catalog = index
        .resolve_object(source, root, limits.resolve)
        .map_err(|error| PageContentError::new(PageContentErrorKind::Resolve(error)))?;
    let catalog_entries = dictionary(catalog.value())
        .ok_or_else(|| PageContentError::new(PageContentErrorKind::CatalogNotDictionary))?;
    let pages = reference_entry(catalog_entries, catalog.source(), b"/Pages")
        .ok_or_else(|| PageContentError::new(PageContentErrorKind::MissingPages))?;
    Ok(PageTree {
        index,
        root: pages,
        catalog: root,
        security,
    })
}

fn open_page_tree_recovering(
    source: &ByteStore,
    limits: PageContentLimits,
    recover: RecoverLimits,
    password: &[u8],
) -> Result<Recovered<PageTree>, PageContentError> {
    match open_page_tree(source, limits, password) {
        Ok(tree) => return Ok(Recovered::new(tree, Vec::new())),
        Err(error) if !error.kind().is_structural() => return Err(error),
        Err(_) => {}
    }
    let mut repairs = Vec::new();
    let (_, header_repairs) = parse_header_recovering(source, recover)
        .map_err(|error| PageContentError::new(PageContentErrorKind::Header(error)))?
        .into_parts();
    repairs.extend(header_repairs);

    let (objects, object_repairs) = open_objects_recovering(source, recover)
        .map_err(|_| PageContentError::new(PageContentErrorKind::RecoveryScanFailed))?
        .into_parts();
    repairs.extend(object_repairs);

    let (index, trailer) = match &objects {
        DocumentObjects::Revisions { chain, index } => (
            index.clone(),
            chain
                .revisions()
                .first()
                .map(|revision| (source.clone(), revision.trailer().clone())),
        ),
        DocumentObjects::Scanned(scanned) => (RevisionIndex::from_scanned(scanned), None),
    };
    let index = Arc::new(index.tolerating_damage().resolving_undefined_as_null());
    repairs.push(Repair::tolerated_structural_damage(0));

    let mut catalog_repairs = Vec::new();
    let named = trailer.as_ref().and_then(|(store, trailer)| {
        let entries = dictionary(trailer)?;
        let root = reference_entry(entries, store, b"/Root")?;
        let resolved = index.resolve_object(source, root, limits.resolve).ok()?;
        dictionary(resolved.value())?;
        catalog_repairs.extend(resolved.repairs().iter().map(|repair| {
            Repair::read_damaged_object(
                resolved.value().span().start(),
                root.object_number(),
                root.generation(),
                *repair,
            )
        }));
        Some((root, resolved.source().clone(), resolved.value().clone()))
    });
    repairs.append(&mut catalog_repairs);
    let (catalog, catalog_source, catalog_value) = if let Some(found) = named {
        found
    } else {
        let found = find_catalog(source, &index, limits)
            .ok_or_else(|| PageContentError::new(PageContentErrorKind::MissingRoot))?;
        repairs.push(Repair::rebuilt_catalog(
            found.1.span().start(),
            found.0.object_number(),
            found.0.generation(),
        ));
        (found.0, found.2, found.1)
    };
    let catalog_entries = dictionary(&catalog_value)
        .ok_or_else(|| PageContentError::new(PageContentErrorKind::CatalogNotDictionary))?;
    let pages = reference_entry(catalog_entries, &catalog_source, b"/Pages")
        .ok_or_else(|| PageContentError::new(PageContentErrorKind::MissingPages))?;
    let _ = password;
    Ok(Recovered::new(
        PageTree {
            index,
            root: pages,
            catalog,
            security: None,
        },
        repairs,
    ))
}

fn find_catalog(
    source: &ByteStore,
    index: &RevisionIndex,
    limits: PageContentLimits,
) -> Option<(Reference, Object, ByteStore)> {
    let mut numbers: Vec<u32> = index
        .selected_entries()
        .map(|entry| entry.entry().object_number())
        .collect();
    numbers.sort_unstable();
    for number in numbers {
        let reference = Reference::new(
            number,
            index.selected_for_number(number)?.entry().generation(),
        );
        let Ok(resolved) = index.resolve_object(source, reference, limits.resolve) else {
            continue;
        };
        let Some(entries) = dictionary(resolved.value()) else {
            continue;
        };
        let is_catalog = entry(entries, resolved.source(), b"/Type")
            .is_some_and(|value| value.name_equals(resolved.source(), b"/Catalog"));
        if is_catalog && entry(entries, resolved.source(), b"/Pages").is_some() {
            return Some((
                reference,
                resolved.value().clone(),
                resolved.source().clone(),
            ));
        }
    }
    None
}

pub fn load_page_program_recovering(
    source: &ByteStore,
    page_index: usize,
    limits: PageContentLimits,
    recover: RecoverLimits,
    password: &[u8],
) -> Result<Recovered<PageProgram>, PageContentError> {
    load_page_program_recovering_for(
        source,
        page_index,
        (limits, recover),
        password,
        Medium::Screen,
    )
}

pub fn load_page_program_recovering_for(
    source: &ByteStore,
    page_index: usize,
    (limits, recover): (PageContentLimits, RecoverLimits),
    password: &[u8],
    medium: Medium,
) -> Result<Recovered<PageProgram>, PageContentError> {
    match load_page_program_for(source, page_index, limits, password, medium) {
        Ok(program) => return Ok(Recovered::new(program, Vec::new())),
        Err(error) if !error.kind().is_structural() => return Err(error),
        Err(_) => {}
    }
    let (tree, mut repairs) =
        open_page_tree_recovering(source, limits, recover, password)?.into_parts();
    let tolerant = tree.tolerating_damage();
    match load_page_program_from_tree(source, tree, page_index, (medium, limits)) {
        Ok(program) => Ok(Recovered::new(program, repairs)),
        Err(error) if !error.kind().is_structural() => Err(error),
        Err(_) => {
            let program =
                load_page_program_from_tree(source, tolerant, page_index, (medium, limits))?;
            repairs.push(Repair::tolerated_structural_damage(0));
            Ok(Recovered::new(program, repairs))
        }
    }
}

pub fn load_page_program_tolerating_damage(
    source: &ByteStore,
    page_index: usize,
    limits: PageContentLimits,
    recover: RecoverLimits,
    password: &[u8],
) -> Result<Recovered<PageProgram>, PageContentError> {
    load_page_program_tolerating_damage_for(
        source,
        page_index,
        (limits, recover),
        password,
        Medium::Screen,
    )
}

pub fn load_page_program_tolerating_damage_for(
    source: &ByteStore,
    page_index: usize,
    (limits, recover): (PageContentLimits, RecoverLimits),
    password: &[u8],
    medium: Medium,
) -> Result<Recovered<PageProgram>, PageContentError> {
    let (tree, mut repairs) =
        open_page_tree_recovering(source, limits, recover, password)?.into_parts();
    let program = load_page_program_from_tree(
        source,
        tree.tolerating_damage(),
        page_index,
        (medium, limits),
    )?;
    repairs.push(Repair::tolerated_structural_damage(0));
    Ok(Recovered::new(program, repairs))
}

pub fn count_pages_recovering(
    source: &ByteStore,
    limits: PageContentLimits,
    recover: RecoverLimits,
    password: &[u8],
) -> Result<Recovered<usize>, PageContentError> {
    match count_pages_with_password(source, limits, password) {
        Ok(count) => return Ok(Recovered::new(count, Vec::new())),
        Err(error) if !error.kind().is_structural() => return Err(error),
        Err(_) => {}
    }
    let (tree, mut repairs) =
        open_page_tree_recovering(source, limits, recover, password)?.into_parts();
    match count_pages_from_tree(source, &tree, limits) {
        Ok(count) => Ok(Recovered::new(count, repairs)),
        Err(error) if !error.kind().is_structural() => Err(error),
        Err(_) => {
            let count = count_pages_from_tree(source, &tree.tolerating_damage(), limits)?;
            repairs.push(Repair::tolerated_structural_damage(0));
            Ok(Recovered::new(count, repairs))
        }
    }
}

pub fn page_references_recovering(
    source: &ByteStore,
    limits: PageContentLimits,
    recover: RecoverLimits,
    password: &[u8],
) -> Result<Recovered<Vec<Reference>>, PageContentError> {
    match page_references_with_password(source, limits, password) {
        Ok(pages) => return Ok(Recovered::new(pages, Vec::new())),
        Err(error) if !error.kind().is_structural() => return Err(error),
        Err(_) => {}
    }
    let (tree, mut repairs) =
        open_page_tree_recovering(source, limits, recover, password)?.into_parts();
    let walk = |tree: &PageTree| {
        let mut pages = Vec::new();
        walk_page_tree(
            source,
            &tree.index,
            tree.root,
            limits,
            (),
            |_, ()| Ok(()),
            |page, ()| {
                pages.push(page);
                Ok(ControlFlow::<()>::Continue(()))
            },
        )?;
        Ok::<_, PageContentError>(pages)
    };
    match walk(&tree) {
        Ok(pages) => Ok(Recovered::new(pages, repairs)),
        Err(error) if !error.kind().is_structural() => Err(error),
        Err(_) => {
            let pages = walk(&tree.tolerating_damage())?;
            repairs.push(Repair::tolerated_structural_damage(0));
            Ok(Recovered::new(pages, repairs))
        }
    }
}

pub fn page_geometries_recovering(
    source: &ByteStore,
    limits: PageContentLimits,
    recover: RecoverLimits,
    password: &[u8],
) -> Result<Recovered<Vec<PageGeometry>>, PageContentError> {
    match page_geometries_with_password(source, limits, password) {
        Ok(geometries) => return Ok(Recovered::new(geometries, Vec::new())),
        Err(error) if !error.kind().is_structural() => return Err(error),
        Err(_) => {}
    }
    let (tree, mut repairs) =
        open_page_tree_recovering(source, limits, recover, password)?.into_parts();
    match page_geometries_from_tree(source, &tree, limits) {
        Ok(geometries) => Ok(Recovered::new(geometries, repairs)),
        Err(error) if !error.kind().is_structural() => Err(error),
        Err(_) => {
            let geometries = page_geometries_from_tree(source, &tree.tolerating_damage(), limits)?;
            repairs.push(Repair::tolerated_structural_damage(0));
            Ok(Recovered::new(geometries, repairs))
        }
    }
}

pub fn load_page_program_with_password(
    source: &ByteStore,
    page_index: usize,
    limits: PageContentLimits,
    password: &[u8],
) -> Result<PageProgram, PageContentError> {
    load_page_program_for(source, page_index, limits, password, Medium::Screen)
}

pub fn load_page_program_for(
    source: &ByteStore,
    page_index: usize,
    limits: PageContentLimits,
    password: &[u8],
    medium: Medium,
) -> Result<PageProgram, PageContentError> {
    let tree = open_page_tree(source, limits, password)?;
    load_page_program_from_tree(source, tree, page_index, (medium, limits))
}

fn load_page_program_from_tree(
    source: &ByteStore,
    tree: PageTree,
    page_index: usize,
    (medium, limits): (Medium, PageContentLimits),
) -> Result<PageProgram, PageContentError> {
    let PageTree {
        index,
        root,
        catalog,
        security,
    } = tree;
    let ownership = Arc::new(ownership::ContentOwnership::new(
        source.clone(),
        Arc::clone(&index),
        root,
        limits,
    ));
    let found = find_page(source, &index, root, page_index, limits)?;
    let page = found.reference;
    let resolved = index
        .resolve_object(source, page, limits.resolve)
        .map_err(|error| PageContentError::new(PageContentErrorKind::Resolve(error)))?;
    let page_entries = dictionary(resolved.value())
        .ok_or_else(|| PageContentError::new(PageContentErrorKind::PageNotDictionary))?;
    let optional_content = Arc::new(load_optional_content(
        source,
        &index,
        catalog,
        (medium, limits),
    )?);
    let resources = load_page_resources(
        source,
        &index,
        security.as_ref(),
        found.resources.as_ref(),
        Arc::clone(&optional_content),
        limits,
    )?;
    let geometry = resolve_page_geometry(source, &index, &found.geometry, limits)?;
    let annotations = annotations::load_annotations(
        &annotations::AnnotationContext {
            document: source,
            index: &index,
            security: security.as_ref(),
            catalog,
            optional_content: Arc::clone(&optional_content),
            limits,
        },
        resolved.source(),
        page_entries,
    );
    let Some(contents) = entry(page_entries, resolved.source(), b"/Contents") else {
        return Ok(PageProgram {
            page,
            geometry,
            streams: Vec::new(),
            resources,
            annotations,
            ownership,
        });
    };
    let content_references = content_stream_references(source, &index, contents, limits)?;
    if content_references.len() > limits.max_content_streams {
        return Err(PageContentError::new(
            PageContentErrorKind::ContentStreamLimit,
        ));
    }
    let mut streams = Vec::with_capacity(content_references.len());
    for content_reference in content_references {
        streams.push(decode_content_stream(
            source,
            &index,
            security.as_deref(),
            content_reference,
            limits,
        )?);
    }
    Ok(PageProgram {
        page,
        geometry,
        streams,
        resources,
        annotations,
        ownership,
    })
}

const ASSUMED_MEDIA_BOX: [f64; 4] = [0.0, 0.0, 612.0, 792.0];

fn resolve_page_geometry(
    source: &ByteStore,
    index: &RevisionIndex,
    inherited: &InheritedGeometry,
    limits: PageContentLimits,
) -> Result<PageGeometry, PageContentError> {
    let (media_box, media_box_span) = match inherited.media_box.as_ref() {
        Some((media_source, media_object)) => (
            page_rectangle(
                source,
                index,
                media_source,
                media_object,
                limits,
                PageContentErrorKind::InvalidMediaBox,
            )?,
            media_object.span(),
        ),
        None if index.damage() == pdf_syntax::Damage::Repair => (
            ASSUMED_MEDIA_BOX,
            source
                .span(0..0)
                .map_err(|_| PageContentError::new(PageContentErrorKind::MissingMediaBox))?,
        ),
        None => return Err(PageContentError::new(PageContentErrorKind::MissingMediaBox)),
    };
    let crop_entry = inherited
        .crop_box
        .as_ref()
        .filter(|(crop_source, crop_object)| {
            !names_nothing(source, index, crop_source, crop_object, limits)
        });
    let (crop_box, crop_box_span) = match crop_entry {
        Some((crop_source, crop_object)) => {
            let crop = page_rectangle(
                source,
                index,
                crop_source,
                crop_object,
                limits,
                PageContentErrorKind::InvalidCropBox,
            )?;
            let intersection = [
                crop[0].max(media_box[0]),
                crop[1].max(media_box[1]),
                crop[2].min(media_box[2]),
                crop[3].min(media_box[3]),
            ];
            if intersection[0] >= intersection[2] || intersection[1] >= intersection[3] {
                return Err(PageContentError::new(PageContentErrorKind::InvalidCropBox));
            }
            (intersection, Some(crop_object.span()))
        }
        None => (media_box, None),
    };
    let (rotate, rotate_span) = match inherited.rotate.as_ref() {
        Some((rotate_source, rotate_object)) => {
            let degrees = resolve_integer(source, index, rotate_source, rotate_object, limits)?;
            if degrees % 90 != 0 {
                return Err(PageContentError::new(PageContentErrorKind::InvalidRotate));
            }
            let normalised = u16::try_from(degrees.rem_euclid(360))
                .map_err(|_| PageContentError::new(PageContentErrorKind::InvalidRotate))?;
            (normalised, Some(rotate_object.span()))
        }
        None => (0, None),
    };
    Ok(PageGeometry {
        media_box,
        media_box_span,
        crop_box,
        crop_box_span,
        rotate,
        rotate_span,
    })
}

fn names_nothing(
    document: &ByteStore,
    index: &RevisionIndex,
    source: &ByteStore,
    value: &Object,
    limits: PageContentLimits,
) -> bool {
    match resolve_indirect(document, index, source, value, limits) {
        Ok((_, resolved)) => matches!(resolved.kind(), ObjectKind::Null),
        Err(error) => match error.kind() {
            PageContentErrorKind::Resolve(resolve) => matches!(
                resolve.kind(),
                pdf_syntax::ResolveErrorKind::MissingObject
                    | pdf_syntax::ResolveErrorKind::FreedObject { .. }
                    | pdf_syntax::ResolveErrorKind::ObjectOffsetOutOfBounds
            ),
            _ => false,
        },
    }
}

fn page_rectangle(
    document: &ByteStore,
    index: &RevisionIndex,
    source: &ByteStore,
    value: &Object,
    limits: PageContentLimits,
    error: PageContentErrorKind,
) -> Result<[f64; 4], PageContentError> {
    let (source, value) = resolve_indirect(document, index, source, value, limits)?;
    let ObjectKind::Array(entries) = value.kind() else {
        return Err(PageContentError::new(error));
    };
    if entries.len() != 4 {
        return Err(PageContentError::new(error));
    }
    let mut corners = [0.0_f64; 4];
    for (slot, entry) in corners.iter_mut().zip(entries) {
        *slot = resolve_number(document, index, &source, entry, limits)?;
    }
    let rectangle = [
        corners[0].min(corners[2]),
        corners[1].min(corners[3]),
        corners[0].max(corners[2]),
        corners[1].max(corners[3]),
    ];
    if rectangle[0] >= rectangle[2] || rectangle[1] >= rectangle[3] {
        return Err(PageContentError::new(error));
    }
    Ok(rectangle)
}

fn six_numbers(source: &ByteStore, value: &Object) -> Option<[f64; 6]> {
    let ObjectKind::Array(entries) = value.kind() else {
        return None;
    };
    if entries.len() != 6 {
        return None;
    }
    let mut numbers = [0.0; 6];
    for (slot, entry) in numbers.iter_mut().zip(entries) {
        if !matches!(entry.kind(), ObjectKind::Number(_)) {
            return None;
        }
        let bytes = source.resolve(entry.span()).ok()?;
        let parsed = std::str::from_utf8(bytes).ok()?.parse::<f64>().ok()?;
        if !parsed.is_finite() {
            return None;
        }
        *slot = parsed;
    }
    Some(numbers)
}

fn resolve_number(
    document: &ByteStore,
    index: &RevisionIndex,
    source: &ByteStore,
    value: &Object,
    limits: PageContentLimits,
) -> Result<f64, PageContentError> {
    let (source, value) = resolve_indirect(document, index, source, value, limits)?;
    if !matches!(value.kind(), ObjectKind::Number(_)) {
        return Err(PageContentError::new(PageContentErrorKind::InvalidMediaBox));
    }
    let bytes = source
        .resolve(value.span())
        .map_err(|_| PageContentError::new(PageContentErrorKind::SourceSpanFailure))?;
    let text = std::str::from_utf8(bytes)
        .map_err(|_| PageContentError::new(PageContentErrorKind::InvalidMediaBox))?;
    let number = text
        .parse::<f64>()
        .map_err(|_| PageContentError::new(PageContentErrorKind::InvalidMediaBox))?;
    if number.is_finite() {
        Ok(number)
    } else {
        Err(PageContentError::new(PageContentErrorKind::InvalidMediaBox))
    }
}

fn resolve_integer(
    document: &ByteStore,
    index: &RevisionIndex,
    source: &ByteStore,
    value: &Object,
    limits: PageContentLimits,
) -> Result<i64, PageContentError> {
    let (source, value) = resolve_indirect(document, index, source, value, limits)?;
    if !matches!(value.kind(), ObjectKind::Number(NumberKind::Integer)) {
        return Err(PageContentError::new(PageContentErrorKind::InvalidRotate));
    }
    let bytes = source
        .resolve(value.span())
        .map_err(|_| PageContentError::new(PageContentErrorKind::SourceSpanFailure))?;
    std::str::from_utf8(bytes)
        .ok()
        .and_then(|text| text.parse::<i64>().ok())
        .ok_or_else(|| PageContentError::new(PageContentErrorKind::InvalidRotate))
}

fn resolve_indirect(
    document: &ByteStore,
    index: &RevisionIndex,
    source: &ByteStore,
    value: &Object,
    limits: PageContentLimits,
) -> Result<(ByteStore, Object), PageContentError> {
    let ObjectKind::Reference(reference) = value.kind() else {
        return Ok((source.clone(), value.clone()));
    };
    let resolved = index
        .resolve_object(document, *reference, limits.resolve)
        .map_err(|error| PageContentError::new(PageContentErrorKind::Resolve(error)))?;
    Ok((resolved.source().clone(), resolved.value().clone()))
}

fn load_optional_content(
    source: &ByteStore,
    index: &RevisionIndex,
    catalog: Reference,
    (medium, limits): (Medium, PageContentLimits),
) -> Result<OptionalContent, PageContentError> {
    let resolved = index
        .resolve_object(source, catalog, limits.resolve)
        .map_err(|error| PageContentError::new(PageContentErrorKind::Resolve(error)))?;
    let Some(entries) = dictionary(resolved.value()) else {
        return Ok(OptionalContent {
            medium,
            ..OptionalContent::default()
        });
    };
    let Some(properties) = entry(entries, resolved.source(), b"/OCProperties") else {
        return Ok(OptionalContent {
            medium,
            ..OptionalContent::default()
        });
    };
    let (properties_source, properties) =
        resolve_direct(source, index, resolved.source(), properties, limits)?;
    let Some(properties_entries) = dictionary(&properties) else {
        return Err(PageContentError::new(
            PageContentErrorKind::InvalidOptionalContent,
        ));
    };
    let mut content = OptionalContent {
        medium,
        ..OptionalContent::default()
    };
    if let Some(groups) = entry(properties_entries, &properties_source, b"/OCGs") {
        let (groups_source, groups) =
            resolve_direct(source, index, &properties_source, groups, limits)?;
        content.known = reference_set(&groups_source, &groups)?;
    }
    if content.known.is_empty() {
        return Ok(content);
    }

    let Configuration {
        base_off,
        on,
        off,
        applied,
    } = configuration(
        source,
        index,
        (&properties_source, properties_entries),
        (medium, limits),
    )?;

    for group in content.known.clone() {
        let Ok(resolved) = index.resolve_object(source, group, limits.resolve) else {
            continue;
        };
        let group_source = resolved.source().clone();
        let entries = dictionary(resolved.value()).unwrap_or_default();
        if !intended_for_view(source, index, &group_source, entries, limits)? {
            continue;
        }
        let own = match entry(entries, &group_source, b"/Usage") {
            Some(usage) => own_state(source, index, &group_source, usage, (medium, limits))?,
            None => None,
        };
        let state = if let Some(state) = own {
            state
        } else {
            let mut state = if base_off {
                ViewState::Off
            } else {
                ViewState::On
            };
            if on.contains(&group) {
                state = ViewState::On;
            }
            if off.contains(&group) {
                state = ViewState::Off;
            }
            for (groups, applied_state) in &applied {
                if groups.contains(&group) {
                    state = *applied_state;
                }
            }
            state
        };
        if state == ViewState::Off {
            content.hidden.insert(group);
        }
    }
    Ok(content)
}

struct Configuration {
    base_off: bool,
    on: HashSet<Reference>,
    off: HashSet<Reference>,
    applied: Vec<(HashSet<Reference>, ViewState)>,
}

fn configuration(
    source: &ByteStore,
    index: &RevisionIndex,
    (properties_source, properties_entries): (&ByteStore, &[pdf_syntax::DictionaryEntry]),
    (medium, limits): (Medium, PageContentLimits),
) -> Result<Configuration, PageContentError> {
    let mut base_off = false;
    let mut on = HashSet::new();
    let mut off = HashSet::new();
    let mut applied: Vec<(HashSet<Reference>, ViewState)> = Vec::new();
    if let Some(default) = entry(properties_entries, properties_source, b"/D") {
        let (default_source, default) =
            resolve_direct(source, index, properties_source, default, limits)?;
        let Some(default_entries) = dictionary(&default) else {
            return Err(PageContentError::new(
                PageContentErrorKind::InvalidOptionalContent,
            ));
        };
        if let Some(base) = entry(default_entries, &default_source, b"/BaseState") {
            if base.name_equals(&default_source, b"/OFF") {
                base_off = true;
            } else if !base.name_equals(&default_source, b"/ON") {
                return Err(PageContentError::new(
                    PageContentErrorKind::InvalidOptionalContent,
                ));
            }
        }
        for (key, into) in [(&b"/ON"[..], &mut on), (&b"/OFF"[..], &mut off)] {
            if let Some(list) = entry(default_entries, &default_source, key) {
                let (list_source, list) =
                    resolve_direct(source, index, &default_source, list, limits)?;
                *into = reference_set(&list_source, &list)?;
            }
        }
        applied = usage_applications(
            source,
            index,
            &default_source,
            default_entries,
            (medium, limits),
        )?;
    }
    Ok(Configuration {
        base_off,
        on,
        off,
        applied,
    })
}

fn own_state(
    document: &ByteStore,
    index: &RevisionIndex,
    source: &ByteStore,
    usage: &Object,
    (medium, limits): (Medium, PageContentLimits),
) -> Result<Option<ViewState>, PageContentError> {
    match usage_state(document, index, source, usage, (medium, limits))? {
        Some(state) => Ok(Some(state)),
        None if medium == Medium::Print => {
            usage_state(document, index, source, usage, (Medium::Screen, limits))
        }
        None => Ok(None),
    }
}

fn usage_applications(
    document: &ByteStore,
    index: &RevisionIndex,
    source: &ByteStore,
    configuration: &[pdf_syntax::DictionaryEntry],
    (medium, limits): (Medium, PageContentLimits),
) -> Result<Vec<(HashSet<Reference>, ViewState)>, PageContentError> {
    let (event, _) = medium.usage_keys();
    let Some(list) = entry(configuration, source, b"/AS") else {
        return Ok(Vec::new());
    };
    let (list_source, list) = resolve_direct(document, index, source, list, limits)?;
    let ObjectKind::Array(items) = list.kind() else {
        return Ok(Vec::new());
    };
    let mut applications = Vec::new();
    for item in items {
        let (item_source, item) = resolve_direct(document, index, &list_source, item, limits)?;
        let Some(entries) = dictionary(&item) else {
            continue;
        };
        let asked = entry(entries, &item_source, b"/Event");
        if !asked.map_or(medium == Medium::Screen, |asked| {
            asked.name_equals(&item_source, event)
        }) {
            continue;
        }
        let Some(groups) = entry(entries, &item_source, b"/OCGs") else {
            continue;
        };
        let (groups_source, groups) =
            resolve_direct(document, index, &item_source, groups, limits)?;
        let Ok(groups) = reference_set(&groups_source, &groups) else {
            continue;
        };
        if let Some(state) = usage_state(document, index, &item_source, &item, (medium, limits))? {
            applications.push((groups, state));
        }
    }
    Ok(applications)
}

fn reference_set(
    source: &ByteStore,
    value: &Object,
) -> Result<HashSet<Reference>, PageContentError> {
    let _ = source;
    let ObjectKind::Array(items) = value.kind() else {
        return Err(PageContentError::new(
            PageContentErrorKind::InvalidOptionalContent,
        ));
    };
    let mut set = HashSet::new();
    for item in items {
        let ObjectKind::Reference(reference) = item.kind() else {
            return Err(PageContentError::new(
                PageContentErrorKind::InvalidOptionalContent,
            ));
        };
        set.insert(*reference);
    }
    Ok(set)
}

fn resolve_direct(
    document: &ByteStore,
    index: &RevisionIndex,
    source: &ByteStore,
    value: &Object,
    limits: PageContentLimits,
) -> Result<(ByteStore, Object), PageContentError> {
    let ObjectKind::Reference(reference) = value.kind() else {
        return Ok((source.clone(), value.clone()));
    };
    let resolved = index
        .resolve_object(document, *reference, limits.resolve)
        .map_err(|error| PageContentError::new(PageContentErrorKind::Resolve(error)))?;
    Ok((resolved.source().clone(), resolved.value().clone()))
}

fn load_page_resources(
    document: &ByteStore,
    index: &Arc<RevisionIndex>,
    security: Option<&Arc<AuthenticatedSecurity>>,
    dictionary: Option<&ResourceDictionary>,
    optional_content: Arc<OptionalContent>,
    limits: PageContentLimits,
) -> Result<PageResources, PageContentError> {
    let binding = Some(Box::new(ResourceBinding {
        document: document.clone(),
        index: Arc::clone(index),
        security: security.map(Arc::clone),
        limits,
    }));
    let Some(dictionary) = dictionary else {
        return Ok(PageResources {
            optional_content,
            binding,
            ..PageResources::default()
        });
    };
    let mut state = ResourceLoadState {
        forms: HashMap::new(),
        visiting: Vec::new(),
        loaded_forms: 0,
        security: security.map(Arc::clone),
        optional_content,
    };
    let mut resources = load_resources(document, index, dictionary, limits, &mut state)?;
    resources.binding = binding;
    Ok(resources)
}

fn load_resources(
    document: &ByteStore,
    index: &Arc<RevisionIndex>,
    dictionary: &ResourceDictionary,
    limits: PageContentLimits,
    state: &mut ResourceLoadState,
) -> Result<PageResources, PageContentError> {
    Ok(PageResources {
        optional_content: Arc::clone(&state.optional_content),
        ext_gstates: load_resource_category(
            document,
            index,
            dictionary,
            b"/ExtGState",
            limits,
            state,
        )?,
        xobjects: load_resource_category(document, index, dictionary, b"/XObject", limits, state)?,
        fonts: load_resource_category(document, index, dictionary, b"/Font", limits, state)?,
        color_spaces: load_resource_category(
            document,
            index,
            dictionary,
            b"/ColorSpace",
            limits,
            state,
        )?,
        patterns: load_resource_category(document, index, dictionary, b"/Pattern", limits, state)?,
        shadings: load_resource_category(document, index, dictionary, b"/Shading", limits, state)?,
        properties: load_resource_category(
            document,
            index,
            dictionary,
            b"/Properties",
            limits,
            state,
        )?,
        binding: None,
    })
}

fn load_tiling_pattern(
    document: &ByteStore,
    index: &Arc<RevisionIndex>,
    security: Option<&Arc<AuthenticatedSecurity>>,
    reference: Reference,
    optional_content: Arc<OptionalContent>,
    limits: PageContentLimits,
) -> Result<TilingPattern, PageContentError> {
    let resolved = index
        .resolve_object(document, reference, limits.resolve)
        .map_err(|error| PageContentError::new(PageContentErrorKind::Resolve(error)))?;
    let entries = dictionary(resolved.value())
        .ok_or_else(|| PageContentError::new(PageContentErrorKind::PatternNotDictionary))?;
    let pattern_type = unique_pattern_entry(entries, resolved.source(), b"/PatternType")?
        .ok_or_else(|| PageContentError::new(PageContentErrorKind::PatternMissingType))?;
    let pattern_type_bytes = resolved
        .source()
        .resolve(pattern_type.span())
        .map_err(|_| PageContentError::new(PageContentErrorKind::SourceSpanFailure))?;
    if !matches!(pattern_type.kind(), ObjectKind::Number(NumberKind::Integer))
        || std::str::from_utf8(pattern_type_bytes)
            .ok()
            .and_then(|text| text.parse::<i64>().ok())
            != Some(1)
    {
        return Err(PageContentError::new(
            PageContentErrorKind::PatternWrongType,
        ));
    }
    let stream = resolved
        .stream()
        .ok_or_else(|| PageContentError::new(PageContentErrorKind::PatternMissingStream))?;
    let encoded = stream_plaintext(security.map(Arc::as_ref), &resolved, reference, stream)?;
    let decoded = decode_stream_bytes(
        resolved.source(),
        entries,
        &encoded,
        stream.data_span().start(),
        limits.max_decoded_stream_bytes,
    )
    .map_err(|error| PageContentError::new(PageContentErrorKind::Decode(error)))?;
    let resources_value = unique_pattern_entry(entries, resolved.source(), b"/Resources")?
        .ok_or_else(|| PageContentError::new(PageContentErrorKind::PatternMissingResources))?;
    let resources_dictionary =
        resolve_resource_dictionary(document, index, resolved.source(), resources_value, limits)?;
    let mut state = ResourceLoadState {
        forms: HashMap::new(),
        visiting: Vec::new(),
        loaded_forms: 0,
        security: security.map(Arc::clone),
        optional_content,
    };
    let resources = load_resources(document, index, &resources_dictionary, limits, &mut state)?;
    let derivation = 0x5054_0000_0000_0000_u64
        | (u64::from(reference.object_number()) << 16)
        | u64::from(reference.generation());
    let bytes = ByteStore::new(
        SourceId::derived(document.id(), derivation),
        Arc::<[u8]>::from(decoded),
    );
    Ok(TilingPattern {
        reference,
        source: resolved.source().clone(),
        dictionary: resolved.value().clone(),
        bytes,
        resources,
    })
}

fn unique_pattern_entry<'a>(
    entries: &'a [pdf_syntax::DictionaryEntry],
    source: &ByteStore,
    key: &[u8],
) -> Result<Option<&'a Object>, PageContentError> {
    let mut matches = entries
        .iter()
        .filter(|entry| entry.key_equals(source, key))
        .map(pdf_syntax::DictionaryEntry::value);
    let first = matches.next();
    if matches.next().is_some() {
        return Err(PageContentError::new(
            PageContentErrorKind::PatternDuplicateEntry,
        ));
    }
    Ok(first)
}

fn load_resource_category(
    document: &ByteStore,
    index: &Arc<RevisionIndex>,
    resources: &ResourceDictionary,
    category: &[u8],
    limits: PageContentLimits,
    state: &mut ResourceLoadState,
) -> Result<Vec<ResourceEntry>, PageContentError> {
    let entries = dictionary(&resources.value)
        .ok_or_else(|| PageContentError::new(PageContentErrorKind::ResourcesNotDictionary))?;
    let Some(category_value) = entry(entries, &resources.source, category) else {
        return Ok(Vec::new());
    };
    let category_dictionary =
        resolve_resource_dictionary(document, index, &resources.source, category_value, limits)?;
    let category_entries = dictionary(&category_dictionary.value).ok_or_else(|| {
        PageContentError::new(PageContentErrorKind::ResourceCategoryNotDictionary)
    })?;
    let mut names = HashSet::new();
    let mut loaded = Vec::with_capacity(category_entries.len());
    for resource in category_entries {
        let name = resource
            .decoded_key(&category_dictionary.source)
            .map_err(|error| PageContentError::new(PageContentErrorKind::ResourceName(error)))?;
        if !names.insert(name.clone()) {
            return Err(PageContentError::new(
                PageContentErrorKind::DuplicateResourceName,
            ));
        }
        let (reference, source, value) = match resource.value().kind() {
            ObjectKind::Reference(reference) => {
                let resolved = index
                    .resolve_object(document, *reference, limits.resolve)
                    .map_err(|error| PageContentError::new(PageContentErrorKind::Resolve(error)))?;
                (
                    Some(*reference),
                    resolved.source().clone(),
                    resolved.value().clone(),
                )
            }
            _ => (
                None,
                category_dictionary.source.clone(),
                resource.value().clone(),
            ),
        };
        let mut form_error = None;
        let form = if category == b"/XObject" && is_form_xobject(&source, &value) {
            let reference = reference
                .ok_or_else(|| PageContentError::new(PageContentErrorKind::FormMustBeIndirect))?;
            match load_form_xobject(
                document,
                index,
                state.security.clone().as_deref(),
                reference,
                limits,
                state,
            ) {
                Ok(form) => Some(form),
                Err(error) if error.kind() == PageContentErrorKind::FormCycle => {
                    form_error = Some(error.kind());
                    None
                }
                Err(error) => return Err(error),
            }
        } else {
            None
        };
        loaded.push(ResourceEntry {
            name,
            reference,
            security: state.security.clone(),
            source,
            value,
            form,
            form_error,
            document: document.clone(),
            index: Arc::clone(index),
            optional_content: Arc::clone(&state.optional_content),
            limits,
        });
    }
    Ok(loaded)
}

fn is_form_xobject(source: &ByteStore, value: &Object) -> bool {
    let Some(entries) = dictionary(value) else {
        return false;
    };
    entry(entries, source, b"/Subtype").is_some_and(|subtype| subtype.name_equals(source, b"/Form"))
}

fn load_form_xobject(
    document: &ByteStore,
    index: &Arc<RevisionIndex>,
    security: Option<&AuthenticatedSecurity>,
    reference: Reference,
    limits: PageContentLimits,
    state: &mut ResourceLoadState,
) -> Result<Arc<FormXObject>, PageContentError> {
    load_form_stream(document, index, security, reference, limits, state, true)
}

fn load_form_stream(
    document: &ByteStore,
    index: &Arc<RevisionIndex>,
    security: Option<&AuthenticatedSecurity>,
    reference: Reference,
    limits: PageContentLimits,
    state: &mut ResourceLoadState,
    require_subtype: bool,
) -> Result<Arc<FormXObject>, PageContentError> {
    if let Some(form) = state.forms.get(&reference) {
        return Ok(Arc::clone(form));
    }
    if state.visiting.contains(&reference) {
        return Err(PageContentError::new(PageContentErrorKind::FormCycle));
    }
    if state.visiting.len() >= limits.max_form_depth {
        return Err(PageContentError::new(PageContentErrorKind::FormDepthLimit));
    }
    if state.loaded_forms >= limits.max_form_xobjects {
        return Err(PageContentError::new(PageContentErrorKind::FormCountLimit));
    }
    state.visiting.push(reference);
    state.loaded_forms += 1;
    let result = (|| {
        let resolved = index
            .resolve_object(document, reference, limits.resolve)
            .map_err(|error| PageContentError::new(PageContentErrorKind::Resolve(error)))?;
        let entries = dictionary(resolved.value())
            .ok_or_else(|| PageContentError::new(PageContentErrorKind::FormNotDictionary))?;
        match entry(entries, resolved.source(), b"/Subtype") {
            Some(subtype) if !subtype.name_equals(resolved.source(), b"/Form") => {
                return Err(PageContentError::new(
                    PageContentErrorKind::FormWrongSubtype,
                ));
            }
            None if require_subtype => {
                return Err(PageContentError::new(
                    PageContentErrorKind::FormMissingSubtype,
                ));
            }
            Some(_) | None => {}
        }
        let stream = resolved
            .stream()
            .ok_or_else(|| PageContentError::new(PageContentErrorKind::FormMissingStream))?;
        let encoded = stream_plaintext(security, &resolved, reference, stream)?;
        let decoded = decode_stream_bytes(
            resolved.source(),
            entries,
            &encoded,
            stream.data_span().start(),
            limits.max_decoded_stream_bytes,
        )
        .map_err(|error| PageContentError::new(PageContentErrorKind::Decode(error)))?;
        let derivation = 0x464d_0000_0000_0000_u64
            | (u64::from(reference.object_number()) << 16)
            | u64::from(reference.generation());
        let bytes = ByteStore::new(
            SourceId::derived(document.id(), derivation),
            Arc::<[u8]>::from(decoded),
        );
        let resources = if let Some(value) = entry(entries, resolved.source(), b"/Resources") {
            let dictionary =
                resolve_resource_dictionary(document, index, resolved.source(), value, limits)?;
            Some(load_resources(document, index, &dictionary, limits, state)?)
        } else {
            None
        };
        Ok(FormXObject {
            reference,
            source: resolved.source().clone(),
            dictionary: resolved.value().clone(),
            bytes,
            resources,
            document: document.clone(),
            index: Arc::clone(index),
            security: state.security.clone(),
            limits,
        })
    })();
    state.visiting.pop();
    let form = Arc::new(result?);
    state.forms.insert(reference, Arc::clone(&form));
    Ok(form)
}

fn resolve_resource_dictionary(
    document: &ByteStore,
    index: &RevisionIndex,
    source: &ByteStore,
    value: &Object,
    limits: PageContentLimits,
) -> Result<ResourceDictionary, PageContentError> {
    match value.kind() {
        ObjectKind::Dictionary(_) => Ok(ResourceDictionary {
            source: source.clone(),
            value: value.clone(),
        }),
        ObjectKind::Reference(reference) => {
            let resolved = index
                .resolve_object(document, *reference, limits.resolve)
                .map_err(|error| PageContentError::new(PageContentErrorKind::Resolve(error)))?;
            if !matches!(resolved.value().kind(), ObjectKind::Dictionary(_)) {
                return Err(PageContentError::new(
                    PageContentErrorKind::ResourcesNotDictionary,
                ));
            }
            Ok(ResourceDictionary {
                source: resolved.source().clone(),
                value: resolved.value().clone(),
            })
        }
        _ => Err(PageContentError::new(
            PageContentErrorKind::ResourcesNotDictionary,
        )),
    }
}

fn content_stream_references(
    source: &ByteStore,
    index: &RevisionIndex,
    contents: &Object,
    limits: PageContentLimits,
) -> Result<Vec<Reference>, PageContentError> {
    let resolved;
    let contents = match contents.kind() {
        ObjectKind::Reference(reference) => {
            let target = index
                .resolve_object(source, *reference, limits.resolve)
                .map_err(|error| PageContentError::new(PageContentErrorKind::Resolve(error)))?;
            if matches!(target.value().kind(), ObjectKind::Array(_)) {
                resolved = target;
                resolved.value()
            } else {
                return Ok(vec![*reference]);
            }
        }
        _ => contents,
    };
    let ObjectKind::Array(entries) = contents.kind() else {
        return Err(PageContentError::new(
            PageContentErrorKind::ContentsNotReference,
        ));
    };
    if entries.len() > limits.max_content_streams {
        return Err(PageContentError::new(
            PageContentErrorKind::ContentStreamLimit,
        ));
    }
    let mut references = Vec::with_capacity(entries.len());
    for content in entries {
        let ObjectKind::Reference(reference) = content.kind() else {
            return Err(PageContentError::new(
                PageContentErrorKind::ContentArrayEntryNotReference,
            ));
        };
        references.push(*reference);
    }
    Ok(references)
}

fn decode_content_stream(
    source: &ByteStore,
    index: &RevisionIndex,
    security: Option<&AuthenticatedSecurity>,
    content_reference: Reference,
    limits: PageContentLimits,
) -> Result<DecodedContentStream, PageContentError> {
    let content = index
        .resolve_object(source, content_reference, limits.resolve)
        .map_err(|error| PageContentError::new(PageContentErrorKind::Resolve(error)))?;
    let dictionary = dictionary(content.value())
        .ok_or_else(|| PageContentError::new(PageContentErrorKind::StreamNotDictionary))?;
    let stream = content
        .stream()
        .ok_or_else(|| PageContentError::new(PageContentErrorKind::ContentsNotStream))?;
    let encoded = stream_plaintext(security, &content, content_reference, stream)?;
    let decoded = decode_stream_bytes_recovering(
        content.source(),
        dictionary,
        &encoded,
        stream.data_span().start(),
        limits.max_decoded_stream_bytes,
    )
    .map_err(|error| PageContentError::new(PageContentErrorKind::Decode(error)))?;
    let derivation = 0x434f_0000_0000_0000_u64
        | (u64::from(content_reference.object_number()) << 16)
        | u64::from(content_reference.generation());
    let bytes = ByteStore::new(
        SourceId::derived(source.id(), derivation),
        Arc::<[u8]>::from(decoded.bytes),
    );
    Ok(DecodedContentStream {
        reference: content_reference,
        bytes,
        repairs: decoded.repairs,
    })
}

fn find_page(
    source: &ByteStore,
    index: &RevisionIndex,
    root: Reference,
    requested: usize,
    limits: PageContentLimits,
) -> Result<FoundPage, PageContentError> {
    let mut remaining = requested;
    let found = walk_page_tree(
        source,
        index,
        root,
        limits,
        (None, InheritedGeometry::default()),
        |node, (inherited_resources, inherited_geometry)| {
            let resources = match dictionary(node.value())
                .and_then(|entries| entry(entries, node.source(), b"/Resources"))
            {
                Some(value) => Some(resolve_resource_dictionary(
                    source,
                    index,
                    node.source(),
                    value,
                    limits,
                )?),
                None => inherited_resources.clone(),
            };
            Ok((resources, inherited_geometry_of(node, inherited_geometry)))
        },
        |reference, (resources, geometry)| {
            if remaining > 0 {
                remaining -= 1;
                return Ok(ControlFlow::Continue(()));
            }
            Ok(ControlFlow::Break(FoundPage {
                reference,
                resources: resources.clone(),
                geometry: geometry.clone(),
            }))
        },
    );
    found?.ok_or_else(|| PageContentError::new(PageContentErrorKind::PageOutOfRange))
}

fn walk_page_tree<State, Outcome>(
    source: &ByteStore,
    index: &RevisionIndex,
    root: Reference,
    limits: PageContentLimits,
    initial: State,
    mut inherit: impl FnMut(&ResolvedObject, &State) -> Result<State, PageContentError>,
    mut leaf: impl FnMut(Reference, &State) -> Result<ControlFlow<Outcome>, PageContentError>,
) -> Result<Option<Outcome>, PageContentError>
where
    State: Clone,
{
    struct Frame<State> {
        node: Reference,
        kids: Vec<Reference>,
        next: usize,
        state: State,
    }

    let mut stack: Vec<Frame<State>> = Vec::new();
    let mut ancestors: HashSet<Reference> = HashSet::new();
    let mut visited = 0_usize;
    let mut pending = Some((root, initial));

    loop {
        let (reference, state) = if let Some(next) = pending.take() {
            next
        } else {
            let Some(frame) = stack.last_mut() else {
                return Ok(None);
            };
            let Some(kid) = frame.kids.get(frame.next).copied() else {
                let frame = stack.pop().expect("the frame just examined");
                ancestors.remove(&frame.node);
                continue;
            };
            frame.next += 1;
            (kid, frame.state.clone())
        };

        if ancestors.contains(&reference) {
            return Err(PageContentError::new(PageContentErrorKind::PageTreeCycle));
        }
        if visited >= limits.max_page_tree_nodes {
            return Err(PageContentError::new(PageContentErrorKind::PageTreeLimit));
        }
        visited += 1;

        let node = index
            .resolve_object(source, reference, limits.resolve)
            .map_err(|error| PageContentError::new(PageContentErrorKind::Resolve(error)))?;
        let entries = dictionary(node.value()).ok_or_else(|| {
            PageContentError::new(PageContentErrorKind::PageTreeNodeNotDictionary)
        })?;
        let node_type = entry(entries, node.source(), b"/Type")
            .ok_or_else(|| PageContentError::new(PageContentErrorKind::MissingPageTreeType))?;
        let is_page = node_type.name_equals(node.source(), b"/Page");
        if !is_page && !node_type.name_equals(node.source(), b"/Pages") {
            return Err(PageContentError::new(
                PageContentErrorKind::InvalidPageTreeType,
            ));
        }

        let state = inherit(&node, &state)?;
        if is_page {
            if let ControlFlow::Break(outcome) = leaf(reference, &state)? {
                return Ok(Some(outcome));
            }
            continue;
        }

        if stack.len() >= limits.max_page_tree_depth {
            return Err(PageContentError::new(PageContentErrorKind::PageTreeLimit));
        }
        let kids = entry(entries, node.source(), b"/Kids")
            .ok_or_else(|| PageContentError::new(PageContentErrorKind::MissingKids))?;
        let ObjectKind::Array(kids) = kids.kind() else {
            return Err(PageContentError::new(PageContentErrorKind::KidsNotArray));
        };
        let kids = kids
            .iter()
            .map(|kid| match kid.kind() {
                ObjectKind::Reference(reference) => Ok(*reference),
                _ => Err(PageContentError::new(PageContentErrorKind::KidNotReference)),
            })
            .collect::<Result<Vec<_>, _>>()?;
        ancestors.insert(reference);
        stack.push(Frame {
            node: reference,
            kids,
            next: 0,
            state,
        });
    }
}

fn dictionary(object: &Object) -> Option<&[pdf_syntax::DictionaryEntry]> {
    let ObjectKind::Dictionary(entries) = object.kind() else {
        return None;
    };
    Some(entries)
}

fn entry<'a>(
    entries: &'a [pdf_syntax::DictionaryEntry],
    source: &ByteStore,
    key: &[u8],
) -> Option<&'a Object> {
    entries
        .iter()
        .find(|candidate| candidate.key_equals(source, key))
        .map(pdf_syntax::DictionaryEntry::value)
}

fn reference_entry(
    entries: &[pdf_syntax::DictionaryEntry],
    source: &ByteStore,
    key: &[u8],
) -> Option<Reference> {
    let ObjectKind::Reference(reference) = entry(entries, source, key)?.kind() else {
        return None;
    };
    Some(*reference)
}

fn declared_style(
    base_font: &[u8],
    declared_weight: Option<i64>,
    flags: FontFlags,
    italic_angle: Option<f64>,
) -> (String, FontStyle) {
    let (family, mut style) = split_family_style(base_font);
    if let Some(weight) = declared_weight
        && (100..=1000).contains(&weight)
    {
        style.weight = u16::try_from(weight).unwrap_or(style.weight);
    }
    if flags.force_bold() && style.weight < 600 {
        style.weight = 700;
    }
    if flags.italic() || italic_angle.is_some_and(|angle| angle.abs() > 4.0) {
        style.italic = true;
    }
    (family, style)
}

const FAMILY_NAME_BYTES: usize = 256;

#[derive(Clone, Debug, Default)]
struct Descriptor {
    flags: FontFlags,
    italic_angle: Option<f64>,
    ascent: Option<f64>,
    descent: Option<f64>,
    declared_weight: Option<i64>,
    present: bool,
    embedded_key: Option<&'static [u8]>,
    font_name: Option<Vec<u8>>,
    font_family: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PageContentError {
    kind: PageContentErrorKind,
}

impl PageContentError {
    const fn new(kind: PageContentErrorKind) -> Self {
        Self { kind }
    }

    #[must_use]
    pub const fn kind(self) -> PageContentErrorKind {
        self.kind
    }
}

impl fmt::Display for PageContentError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.kind.fmt(formatter)
    }
}

impl std::error::Error for PageContentError {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PageContentErrorKind {
    Header(HeaderError),
    Revisions(XrefError),
    Resolve(ResolveError),
    Decode(StreamDecodeError),
    MissingRevision,
    TrailerNotDictionary,
    Encrypted,
    MissingRoot,
    CatalogNotDictionary,
    MissingPages,
    PageTreeNodeNotDictionary,
    MissingPageTreeType,
    InvalidPageTreeType,
    MissingKids,
    KidsNotArray,
    KidNotReference,
    PageTreeCycle,
    PageTreeLimit,
    PageOutOfRange,
    PageNotDictionary,
    ResourcesNotDictionary,
    ResourceCategoryNotDictionary,
    ResourceName(NameDecodeError),
    DuplicateResourceName,
    FormMustBeIndirect,
    FormNotDictionary,
    FormMissingSubtype,
    FormWrongSubtype,
    FormMissingStream,
    FormCycle,
    FormDepthLimit,
    FormCountLimit,
    PatternMustBeIndirect,
    PatternNotDictionary,
    PatternMissingType,
    PatternDuplicateEntry,
    PatternWrongType,
    PatternMissingShading,
    InvalidOptionalContent,
    RecoveryScanFailed,
    PatternMissingStream,
    PatternMissingResources,
    Type3MissingFontMatrix,
    Type3InvalidFontMatrix,
    Type3CharProcsNotDictionary,
    Type3ProcedureNotStream,
    Type3ProcedureLimit,
    IccProfileNotDictionary,
    IccProfileMissingStream,
    MissingMediaBox,
    InvalidMediaBox,
    InvalidCropBox,
    InvalidRotate,
    AnnotsNotArray,
    AnnotationNotDictionary,
    InvalidAnnotationRect,
    AppearanceNotStream,
    AnnotationLimit,
    IndexedLookupNotDictionary,
    IndexedLookupMissingStream,
    FunctionNotDictionary,
    FontProgram(GlyphProgramError),
    ImageNotDictionary,
    ImageMissingStream,
    Decrypt(SecurityErrorKind),
    Authenticate(SecurityErrorKind),
    ContentArrayEntryNotReference,
    ContentStreamLimit,
    ContentsNotReference,
    StreamNotDictionary,
    ContentsNotStream,
    SourceSpanFailure,
}

impl fmt::Display for PageContentErrorKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Header(error) => write!(formatter, "header: {error}"),
            Self::Revisions(error) => write!(formatter, "revision chain: {error}"),
            Self::Resolve(error) => write!(formatter, "object resolution: {error}"),
            Self::Decode(error) => write!(formatter, "content decoding: {error}"),
            Self::ResourceName(error) => write!(formatter, "resource name: {error}"),
            Self::Decrypt(error) => write!(formatter, "object decryption: {error}"),
            Self::Authenticate(error) => write!(formatter, "document authentication: {error}"),
            Self::FontProgram(error) => write!(formatter, "font program: {error}"),
            other => formatter.write_str(other.message()),
        }
    }
}

impl PageContentErrorKind {
    #[must_use]
    pub const fn is_structural(&self) -> bool {
        matches!(
            self,
            Self::Header(_)
                | Self::Revisions(_)
                | Self::Resolve(_)
                | Self::MissingRevision
                | Self::TrailerNotDictionary
                | Self::MissingRoot
                | Self::CatalogNotDictionary
                | Self::MissingPages
                | Self::PageTreeNodeNotDictionary
                | Self::MissingPageTreeType
                | Self::InvalidPageTreeType
                | Self::MissingKids
                | Self::KidsNotArray
                | Self::KidNotReference
                | Self::PageTreeCycle
                | Self::PageNotDictionary
                | Self::MissingMediaBox
        )
    }

    fn message(&self) -> &'static str {
        match self {
            Self::Header(_)
            | Self::Revisions(_)
            | Self::Resolve(_)
            | Self::Decode(_)
            | Self::ResourceName(_)
            | Self::Decrypt(_)
            | Self::Authenticate(_)
            | Self::FontProgram(_) => "page-content error carries its own message",
            Self::MissingRevision => "revision chain is empty",
            Self::TrailerNotDictionary => "trailer is not a dictionary",
            Self::Encrypted => "encrypted page content is not connected yet",
            Self::MissingRoot => "trailer has no indirect /Root",
            Self::CatalogNotDictionary => "catalog is not a dictionary",
            Self::MissingPages => "catalog has no indirect /Pages",
            Self::PageTreeNodeNotDictionary => "page-tree node is not a dictionary",
            Self::MissingPageTreeType => "page-tree node has no /Type",
            Self::InvalidPageTreeType => "page-tree node has invalid /Type",
            Self::MissingKids => "/Pages node has no /Kids",
            Self::KidsNotArray => "page-tree /Kids is not an array",
            Self::KidNotReference => "page-tree kid is not an indirect reference",
            Self::PageTreeCycle => "page tree contains a reference cycle",
            Self::PageTreeLimit => "page-tree node limit exceeded",
            Self::PageOutOfRange => "requested page is out of range",
            Self::PageNotDictionary => "page object is not a dictionary",
            Self::ResourcesNotDictionary => "inherited /Resources is not a dictionary",
            Self::ResourceCategoryNotDictionary => "resource category is not a dictionary",
            Self::DuplicateResourceName => "resource category contains a duplicate name",
            Self::FormMustBeIndirect => "Form XObject must be an indirect stream",
            Self::FormNotDictionary => "Form XObject is not a dictionary",
            Self::FormMissingSubtype => "Form XObject has no /Subtype",
            Self::FormWrongSubtype => "XObject is not a Form",
            Self::FormMissingStream => "Form XObject has no stream data",
            Self::FormCycle => "Form XObject resources contain a cycle",
            Self::FormDepthLimit => "Form XObject depth limit exceeded",
            Self::FormCountLimit => "Form XObject count limit exceeded",
            Self::PatternMustBeIndirect => "tiling pattern must be an indirect stream",
            Self::PatternNotDictionary => "pattern is not a dictionary",
            Self::PatternMissingType => "pattern has no /PatternType",
            Self::PatternDuplicateEntry => "tiling pattern contains a duplicate required entry",
            Self::PatternWrongType => "pattern is not of a kind this engine draws",
            Self::PatternMissingShading => "shading pattern has no /Shading",
            Self::InvalidOptionalContent => "/OCProperties is malformed",
            Self::RecoveryScanFailed => "the recovery scan could not rebuild an object map",
            Self::PatternMissingStream => "tiling pattern has no stream data",
            Self::PatternMissingResources => "tiling pattern has no /Resources",
            Self::Type3MissingFontMatrix => "Type 3 font has no /FontMatrix",
            Self::Type3InvalidFontMatrix => "Type 3 /FontMatrix is not six numbers",
            Self::Type3CharProcsNotDictionary => "Type 3 /CharProcs is not a dictionary",
            Self::Type3ProcedureNotStream => "Type 3 glyph procedure is not a stream",
            Self::Type3ProcedureLimit => {
                "Type 3 font declares more glyph procedures than the limit"
            }
            Self::IccProfileNotDictionary => "ICC profile stream has no dictionary",
            Self::IccProfileMissingStream => "ICC profile object has no stream data",
            Self::MissingMediaBox => "page tree declares no /MediaBox",
            Self::InvalidMediaBox => "/MediaBox is not a usable rectangle",
            Self::InvalidCropBox => "/CropBox is not a usable rectangle inside the media box",
            Self::InvalidRotate => "/Rotate is not a multiple of 90",
            Self::AnnotsNotArray => "/Annots is not an array",
            Self::AnnotationNotDictionary => "an /Annots entry is not a dictionary",
            Self::InvalidAnnotationRect => "annotation /Rect is not a rectangle with area",
            Self::AppearanceNotStream => "annotation appearance is not a stream",
            Self::AnnotationLimit => "annotation or form field count exceeds the configured limit",
            Self::IndexedLookupNotDictionary => "Indexed lookup stream has no dictionary",
            Self::IndexedLookupMissingStream => "Indexed lookup object has no stream data",
            Self::FunctionNotDictionary => "function object has no dictionary",
            Self::ImageNotDictionary => "image XObject has no dictionary",
            Self::ImageMissingStream => "image XObject has no stream data",
            Self::ContentArrayEntryNotReference => {
                "page content-array entry is not an indirect reference"
            }
            Self::ContentStreamLimit => "page content-stream limit exceeded",
            Self::ContentsNotReference => "page /Contents is not an indirect reference",
            Self::StreamNotDictionary => "content stream has no dictionary",
            Self::ContentsNotStream => "page /Contents object is not a stream",
            Self::SourceSpanFailure => "content stream span cannot be resolved",
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use pdf_bytes::{ByteStore, SourceId};
    use pdf_syntax::ObjectKind;

    use super::{
        PageContentErrorKind, PageContentLimits, count_pages_strict, load_page_program_strict,
        load_page_program_with_password,
    };

    fn fixture(contents: &[u8], contents_entry: &[u8]) -> ByteStore {
        fixture_with_pages(contents, contents_entry, b"")
    }

    fn fixture_with_pages(contents: &[u8], contents_entry: &[u8], pages_entry: &[u8]) -> ByteStore {
        let mut bytes = b"%PDF-1.7\n".to_vec();
        let catalog = bytes.len();
        bytes.extend_from_slice(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");
        let pages = bytes.len();
        bytes.extend_from_slice(
            b"2 0 obj\n<< /Type /Pages /MediaBox [0 0 200 100] /Kids [3 0 R] /Count 1 ",
        );
        bytes.extend_from_slice(pages_entry);
        bytes.extend_from_slice(b" >>\nendobj\n");
        let page = bytes.len();
        bytes.extend_from_slice(b"3 0 obj\n<< /Type /Page /Parent 2 0 R ");
        bytes.extend_from_slice(contents_entry);
        bytes.extend_from_slice(b" >>\nendobj\n");
        let stream = bytes.len();
        bytes.extend_from_slice(b"4 0 obj\n<< /Length ");
        bytes.extend_from_slice(contents.len().to_string().as_bytes());
        bytes.extend_from_slice(b" >>\nstream\n");
        bytes.extend_from_slice(contents);
        bytes.extend_from_slice(b"\nendstream\nendobj\n");
        let xref = bytes.len();
        bytes.extend_from_slice(b"xref\n0 5\n0000000000 65535 f \n");
        for offset in [catalog, pages, page, stream] {
            bytes.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
        }
        bytes.extend_from_slice(b"trailer\n<< /Size 5 /Root 1 0 R >>\nstartxref\n");
        bytes.extend_from_slice(xref.to_string().as_bytes());
        bytes.extend_from_slice(b"\n%%EOF\n");
        ByteStore::new(SourceId::new(33), Arc::<[u8]>::from(bytes))
    }

    fn tree_fixture(objects: &[&[u8]]) -> ByteStore {
        let mut bytes = b"%PDF-1.7\n".to_vec();
        let mut offsets = Vec::new();
        for (index, body) in objects.iter().enumerate() {
            offsets.push(bytes.len());
            bytes.extend_from_slice(format!("{} 0 obj\n", index + 1).as_bytes());
            bytes.extend_from_slice(body);
            bytes.extend_from_slice(b"\nendobj\n");
        }
        let xref = bytes.len();
        bytes.extend_from_slice(
            format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes(),
        );
        for offset in &offsets {
            bytes.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
        }
        bytes.extend_from_slice(
            format!(
                "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
                objects.len() + 1
            )
            .as_bytes(),
        );
        ByteStore::new(SourceId::new(34), Arc::<[u8]>::from(bytes))
    }

    fn form_fixture(form_resources: &[u8]) -> ByteStore {
        let page_content = b"/F1 Do /F2 Do";
        let form_content = b"/GS1 gs 0 0 2 3 re f";
        let mut bytes = b"%PDF-1.7\n".to_vec();
        let mut offsets = Vec::new();
        offsets.push(bytes.len());
        bytes.extend_from_slice(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");
        offsets.push(bytes.len());
        bytes.extend_from_slice(
            b"2 0 obj\n<< /Type /Pages /MediaBox [0 0 200 100] /Kids [3 0 R] /Count 1 >>\nendobj\n",
        );
        offsets.push(bytes.len());
        bytes.extend_from_slice(b"3 0 obj\n<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /XObject << /F1 5 0 R /F2 5 0 R >> >> >>\nendobj\n");
        offsets.push(bytes.len());
        bytes.extend_from_slice(b"4 0 obj\n<< /Length ");
        bytes.extend_from_slice(page_content.len().to_string().as_bytes());
        bytes.extend_from_slice(b" >>\nstream\n");
        bytes.extend_from_slice(page_content);
        bytes.extend_from_slice(b"\nendstream\nendobj\n");
        offsets.push(bytes.len());
        bytes.extend_from_slice(b"5 0 obj\n<< /Type /XObject /Subtype /Form /BBox [0 0 2 3] ");
        bytes.extend_from_slice(form_resources);
        bytes.extend_from_slice(b" /Length ");
        bytes.extend_from_slice(form_content.len().to_string().as_bytes());
        bytes.extend_from_slice(b" >>\nstream\n");
        bytes.extend_from_slice(form_content);
        bytes.extend_from_slice(b"\nendstream\nendobj\n");
        let xref = bytes.len();
        bytes.extend_from_slice(b"xref\n0 6\n0000000000 65535 f \n");
        for offset in offsets {
            bytes.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
        }
        bytes.extend_from_slice(b"trailer\n<< /Size 6 /Root 1 0 R >>\nstartxref\n");
        bytes.extend_from_slice(xref.to_string().as_bytes());
        bytes.extend_from_slice(b"\n%%EOF\n");
        ByteStore::new(SourceId::new(34), Arc::<[u8]>::from(bytes))
    }

    fn indirect_widths_fixture() -> ByteStore {
        let content = b"BT /F1 12 Tf (A) Tj ET";
        let mut bytes = b"%PDF-1.7\n".to_vec();
        let mut offsets = Vec::new();
        offsets.push(bytes.len());
        bytes.extend_from_slice(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");
        offsets.push(bytes.len());
        bytes.extend_from_slice(
            b"2 0 obj\n<< /Type /Pages /MediaBox [0 0 200 100] /Kids [3 0 R] /Count 1 >>\nendobj\n",
        );
        offsets.push(bytes.len());
        bytes.extend_from_slice(b"3 0 obj\n<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>\nendobj\n");
        offsets.push(bytes.len());
        bytes.extend_from_slice(b"4 0 obj\n<< /Length ");
        bytes.extend_from_slice(content.len().to_string().as_bytes());
        bytes.extend_from_slice(b" >>\nstream\n");
        bytes.extend_from_slice(content);
        bytes.extend_from_slice(b"\nendstream\nendobj\n");
        offsets.push(bytes.len());
        bytes.extend_from_slice(b"5 0 obj\n<< /Type /Font /Subtype /TrueType /BaseFont /Arial /FirstChar 65 /LastChar 66 /Widths 6 0 R >>\nendobj\n");
        offsets.push(bytes.len());
        bytes.extend_from_slice(b"6 0 obj\n[ 600 700 ]\nendobj\n");
        let xref = bytes.len();
        bytes.extend_from_slice(b"xref\n0 7\n0000000000 65535 f \n");
        for offset in offsets {
            bytes.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
        }
        bytes.extend_from_slice(b"trailer\n<< /Size 7 /Root 1 0 R >>\nstartxref\n");
        bytes.extend_from_slice(xref.to_string().as_bytes());
        bytes.extend_from_slice(b"\n%%EOF\n");
        ByteStore::new(SourceId::new(35), Arc::<[u8]>::from(bytes))
    }

    #[test]
    fn font_metrics_follow_one_indirect_entry() {
        let source = indirect_widths_fixture();
        let page = load_page_program_strict(&source, 0, PageContentLimits::default())
            .expect("page with an indirect /Widths");
        let resource = page.resources.font(b"/F1").expect("font resource");
        let font = resource.simple_font().expect("indirect widths resolve");
        assert_eq!(font.subtype(), b"/TrueType");
        assert!((font.width(65) - 600.0).abs() < f64::EPSILON);
        assert!((font.width(66) - 700.0).abs() < f64::EPSILON);
    }

    fn hex(text: &str) -> Vec<u8> {
        (0..text.len() / 2)
            .map(|index| u8::from_str_radix(&text[index * 2..index * 2 + 2], 16).expect("hex"))
            .collect()
    }

    fn encrypted_fixture(content_ciphertext: &[u8]) -> ByteStore {
        const OWNER: &str = "2055c756c72e1ad702608e8196acad447ad32d17cff583235f6dd15fed7dab67";
        const USER: &str = "b6271bb74f4fd4bf931172dcde8682912edc27b84ad0dc7cb83dc19fb91734d5";
        let mut bytes = b"%PDF-1.4\n".to_vec();
        let mut offsets = Vec::new();
        offsets.push(bytes.len());
        bytes.extend_from_slice(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");
        offsets.push(bytes.len());
        bytes.extend_from_slice(
            b"2 0 obj\n<< /Type /Pages /MediaBox [0 0 200 100] /Kids [3 0 R] /Count 1 >>\nendobj\n",
        );
        offsets.push(bytes.len());
        bytes.extend_from_slice(
            b"3 0 obj\n<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << >> >>\nendobj\n",
        );
        offsets.push(bytes.len());
        bytes.extend_from_slice(b"4 0 obj\n<< /Length ");
        bytes.extend_from_slice(content_ciphertext.len().to_string().as_bytes());
        bytes.extend_from_slice(b" >>\nstream\n");
        bytes.extend_from_slice(content_ciphertext);
        bytes.extend_from_slice(b"\nendstream\nendobj\n");
        offsets.push(bytes.len());
        bytes.extend_from_slice(b"5 0 obj\n<< /Filter /Standard /V 1 /R 2 /Length 40 /P -4 /O <");
        bytes.extend_from_slice(OWNER.as_bytes());
        bytes.extend_from_slice(b"> /U <");
        bytes.extend_from_slice(USER.as_bytes());
        bytes.extend_from_slice(b"> >>\nendobj\n");
        let xref = bytes.len();
        bytes.extend_from_slice(b"xref\n0 6\n0000000000 65535 f \n");
        for offset in offsets {
            bytes.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
        }
        bytes.extend_from_slice(
            b"trailer\n<< /Size 6 /Root 1 0 R /Encrypt 5 0 R /ID [<000102030405060708090a0b0c0d0e0f> <000102030405060708090a0b0c0d0e0f>] >>\nstartxref\n",
        );
        bytes.extend_from_slice(xref.to_string().as_bytes());
        bytes.extend_from_slice(b"\n%%EOF\n");
        ByteStore::new(SourceId::new(41), Arc::<[u8]>::from(bytes))
    }

    #[test]
    fn an_encrypted_page_decrypts_its_content_stream_before_its_filters() {
        let ciphertext = hex("47631e9a29832b578cbe29b2f278bd981557127f16ffb2c649");
        let source = encrypted_fixture(&ciphertext);
        let page = load_page_program_strict(&source, 0, PageContentLimits::default())
            .expect("empty user password opens the document");
        assert_eq!(page.streams.len(), 1);
        assert_eq!(
            page.streams[0].bytes.as_bytes(),
            b"0 0 0 rg 10 20 30 40 re f"
        );
        assert_ne!(page.streams[0].bytes.id(), source.id());
    }

    #[test]
    fn a_wrong_credential_fails_instead_of_producing_plaintext_shaped_bytes() {
        let ciphertext = hex("47631e9a29832b578cbe29b2f278bd981557127f16ffb2c649");
        let source = encrypted_fixture(&ciphertext);
        let error = load_page_program_with_password(
            &source,
            0,
            PageContentLimits::default(),
            b"not the password",
        )
        .expect_err("a wrong credential must fail");
        assert!(matches!(
            error.kind(),
            PageContentErrorKind::Authenticate(_)
        ));
    }

    #[test]
    fn resolves_and_decodes_the_first_page_content_stream() {
        let source = fixture(b"10 20 30 40 re f", b"/Contents 4 0 R");
        let page = load_page_program_strict(&source, 0, PageContentLimits::default())
            .expect("first page content");
        assert_eq!(page.page, pdf_syntax::Reference::new(3, 0));
        assert_eq!(page.streams.len(), 1);
        assert_eq!(page.streams[0].reference, pdf_syntax::Reference::new(4, 0));
        assert_eq!(page.streams[0].bytes.as_bytes(), b"10 20 30 40 re f");
        assert_ne!(page.streams[0].bytes.id(), source.id());
    }

    #[test]
    fn inherits_page_tree_resources_and_decodes_resource_names() {
        let source = fixture_with_pages(
            b"/GS1 gs",
            b"/Contents 4 0 R",
            b"/Resources << /ExtGState << /GS#31 << /Type /ExtGState /LW 4 >> >> /XObject << /Fm 4 0 R >> >>",
        );
        let page = load_page_program_strict(&source, 0, PageContentLimits::default())
            .expect("inherited resources");
        let state = page
            .resources
            .ext_gstate(b"/GS1")
            .expect("escaped ExtGState name");
        assert_eq!(state.name(), b"/GS1");
        assert!(matches!(state.value().kind(), ObjectKind::Dictionary(_)));
        assert_eq!(
            page.resources.xobject(b"/Fm").unwrap().reference(),
            Some(pdf_syntax::Reference::new(4, 0))
        );
    }

    #[test]
    fn decodes_each_form_definition_once_and_retains_its_resource_scope() {
        let source =
            form_fixture(b"/Resources << /ExtGState << /GS1 << /Type /ExtGState /ca .5 >> >> >>");
        let page = load_page_program_strict(&source, 0, PageContentLimits::default())
            .expect("page and Form resources");
        let first = page.resources.xobject(b"/F1").unwrap().form().unwrap();
        let second = page.resources.xobject(b"/F2").unwrap().form().unwrap();
        assert_eq!(first.reference, pdf_syntax::Reference::new(5, 0));
        assert_eq!(first.bytes.as_bytes(), b"/GS1 gs 0 0 2 3 re f");
        assert_ne!(first.bytes.id(), source.id());
        assert!(
            first
                .resources
                .as_ref()
                .unwrap()
                .ext_gstate(b"/GS1")
                .is_some()
        );
        assert!(std::ptr::eq(first, second));
    }

    #[test]
    fn blank_pages_and_content_arrays_preserve_declared_sequence() {
        let blank = fixture(b"", b"");
        let page =
            load_page_program_strict(&blank, 0, PageContentLimits::default()).expect("blank page");
        assert!(page.streams.is_empty());

        let array = fixture(b"1 2 m", b"/Contents [4 0 R 4 0 R]");
        let page = load_page_program_strict(&array, 0, PageContentLimits::default())
            .expect("content sequence");
        assert_eq!(page.streams.len(), 2);
        assert_eq!(page.streams[0].reference, pdf_syntax::Reference::new(4, 0));
        assert_eq!(page.streams[1].bytes.as_bytes(), b"1 2 m");
    }

    #[test]
    fn a_page_named_twice_by_one_parent_is_two_pages_and_not_a_cycle() {
        let source = tree_fixture(&[
            b"<< /Type /Catalog /Pages 2 0 R >>",
            b"<< /Type /Pages /Count 3 /Kids [3 0 R 3 0 R 3 0 R] >>",
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 100] >>",
        ]);
        assert_eq!(
            count_pages_strict(&source, PageContentLimits::default()).expect("three mentions"),
            3
        );
        for index in 0..3 {
            let page = load_page_program_strict(&source, index, PageContentLimits::default())
                .expect("each mention is a page");
            assert!(page.streams.is_empty());
        }
        assert_eq!(
            load_page_program_strict(&source, 3, PageContentLimits::default())
                .expect_err("and no more")
                .kind(),
            PageContentErrorKind::PageOutOfRange
        );
    }

    #[test]
    fn a_page_shared_by_two_parents_is_reached_through_both() {
        let source = tree_fixture(&[
            b"<< /Type /Catalog /Pages 2 0 R >>",
            b"<< /Type /Pages /Count 2 /Kids [3 0 R 4 0 R] >>",
            b"<< /Type /Pages /Parent 2 0 R /Count 1 /Kids [5 0 R] >>",
            b"<< /Type /Pages /Parent 2 0 R /Count 1 /Kids [5 0 R] >>",
            b"<< /Type /Page /MediaBox [0 0 200 100] >>",
        ]);
        assert_eq!(
            count_pages_strict(&source, PageContentLimits::default()).expect("two paths"),
            2
        );
    }

    #[test]
    fn a_node_that_is_its_own_ancestor_is_a_cycle_and_ends_the_document() {
        let source = tree_fixture(&[
            b"<< /Type /Catalog /Pages 2 0 R >>",
            b"<< /Type /Pages /Count 1 /Kids [2 0 R] >>",
        ]);
        assert_eq!(
            count_pages_strict(&source, PageContentLimits::default())
                .expect_err("a cycle")
                .kind(),
            PageContentErrorKind::PageTreeCycle
        );

        let indirect = tree_fixture(&[
            b"<< /Type /Catalog /Pages 2 0 R >>",
            b"<< /Type /Pages /Count 1 /Kids [3 0 R] >>",
            b"<< /Type /Pages /Count 1 /Kids [2 0 R] >>",
        ]);
        assert_eq!(
            count_pages_strict(&indirect, PageContentLimits::default())
                .expect_err("a cycle")
                .kind(),
            PageContentErrorKind::PageTreeCycle
        );
    }

    #[test]
    fn a_tree_deeper_than_the_bound_is_refused_rather_than_walked() {
        let source = tree_fixture(&[
            b"<< /Type /Catalog /Pages 2 0 R >>",
            b"<< /Type /Pages /Count 1 /Kids [3 0 R] >>",
            b"<< /Type /Pages /Count 1 /Kids [4 0 R] >>",
            b"<< /Type /Page /MediaBox [0 0 200 100] >>",
        ]);
        assert_eq!(
            count_pages_strict(&source, PageContentLimits::default()).expect("two levels"),
            1
        );
        assert_eq!(
            count_pages_strict(
                &source,
                PageContentLimits {
                    max_page_tree_depth: 1,
                    ..PageContentLimits::default()
                }
            )
            .expect_err("deeper than the bound")
            .kind(),
            PageContentErrorKind::PageTreeLimit
        );
    }

    #[test]
    fn page_range_and_tree_limits_are_checked() {
        let source = fixture(b"", b"");
        let range = load_page_program_strict(&source, 1, PageContentLimits::default())
            .expect_err("only one page");
        assert_eq!(range.kind(), PageContentErrorKind::PageOutOfRange);

        let limit = load_page_program_strict(
            &source,
            0,
            PageContentLimits {
                max_page_tree_nodes: 1,
                ..PageContentLimits::default()
            },
        )
        .expect_err("tree bound");
        assert_eq!(limit.kind(), PageContentErrorKind::PageTreeLimit);
    }

    fn indirect_array_fixture(first: &[u8], second: &[u8]) -> ByteStore {
        let mut bytes = b"%PDF-1.7\n".to_vec();
        let mut offsets = Vec::new();
        offsets.push(bytes.len());
        bytes.extend_from_slice(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");
        offsets.push(bytes.len());
        bytes.extend_from_slice(
            b"2 0 obj\n<< /Type /Pages /MediaBox [0 0 200 100] /Kids [3 0 R] /Count 1 >>\nendobj\n",
        );
        offsets.push(bytes.len());
        bytes.extend_from_slice(
            b"3 0 obj\n<< /Type /Page /Parent 2 0 R /Contents 6 0 R >>\nendobj\n",
        );
        for content in [first, second] {
            offsets.push(bytes.len());
            let number = offsets.len();
            bytes.extend_from_slice(format!("{number} 0 obj\n<< /Length ").as_bytes());
            bytes.extend_from_slice(content.len().to_string().as_bytes());
            bytes.extend_from_slice(b" >>\nstream\n");
            bytes.extend_from_slice(content);
            bytes.extend_from_slice(b"\nendstream\nendobj\n");
        }
        offsets.push(bytes.len());
        bytes.extend_from_slice(b"6 0 obj\n[4 0 R 5 0 R]\nendobj\n");
        let size = offsets.len() + 1;
        let xref = bytes.len();
        bytes.extend_from_slice(format!("xref\n0 {size}\n0000000000 65535 f \n").as_bytes());
        for offset in offsets {
            bytes.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
        }
        bytes.extend_from_slice(
            format!("trailer\n<< /Size {size} /Root 1 0 R >>\nstartxref\n").as_bytes(),
        );
        bytes.extend_from_slice(xref.to_string().as_bytes());
        bytes.extend_from_slice(b"\n%%EOF\n");
        ByteStore::new(SourceId::new(34), Arc::<[u8]>::from(bytes))
    }

    #[test]
    fn a_contents_reference_may_name_the_array_rather_than_a_stream() {
        let source = indirect_array_fixture(b"1 0 0 1 0 0 cm", b"0 0 1 1 re f");
        let page = load_page_program_strict(&source, 0, PageContentLimits::default())
            .expect("a page whose /Contents reference names an array");
        assert_eq!(page.streams.len(), 2, "both streams, in the array's order");
        assert_eq!(
            page.streams[0].bytes.as_bytes(),
            b"1 0 0 1 0 0 cm",
            "the first element first"
        );
        assert_eq!(page.streams[1].bytes.as_bytes(), b"0 0 1 1 re f");

        let direct = fixture(b"0 0 1 1 re f", b"/Contents [4 0 R]");
        let page = load_page_program_strict(&direct, 0, PageContentLimits::default())
            .expect("a page whose /Contents is a direct array");
        assert_eq!(page.streams.len(), 1);
        assert_eq!(page.streams[0].bytes.as_bytes(), b"0 0 1 1 re f");
    }

    #[test]
    fn an_array_element_that_is_not_a_reference_is_still_refused() {
        let source = fixture(b"0 0 1 1 re f", b"/Contents [<< /Length 0 >>]");
        let error = load_page_program_strict(&source, 0, PageContentLimits::default())
            .expect_err("a direct stream inside the array");
        assert_eq!(
            error.kind(),
            PageContentErrorKind::ContentArrayEntryNotReference
        );
    }

    #[test]
    #[allow(clippy::float_cmp)]
    fn a_crop_box_that_names_no_object_leaves_the_media_box_standing() {
        let source = fixture(b"0 0 1 1 re f", b"/Contents 4 0 R /CropBox 0 0 R");
        let program = load_page_program_strict(&source, 0, PageContentLimits::default())
            .expect("a null /CropBox is not a broken one");
        assert_eq!(program.geometry.crop_box, [0.0, 0.0, 200.0, 100.0]);
        assert_eq!(program.geometry.crop_box, program.geometry.media_box);

        let outside = fixture(
            b"0 0 1 1 re f",
            b"/Contents 4 0 R /CropBox [210 310 215 330]",
        );
        load_page_program_strict(&outside, 0, PageContentLimits::default())
            .expect_err("a crop box that misses the media box entirely is not a page");
    }

    #[test]
    fn a_broken_cross_reference_is_rebuilt_and_the_repairs_come_back() {
        let healthy = fixture(b"0 0 1 1 re f", b"/Contents 4 0 R");
        let mut damaged = healthy.as_bytes().to_vec();
        let xref = find(&damaged, b"xref\n0 5\n").expect("xref");
        let row = xref + b"xref\n0 5\n".len() + b"0000000000 65535 f \n".len();
        damaged[row..row + 10].copy_from_slice(b"00000000x0");
        let damaged = ByteStore::new(SourceId::new(35), Arc::<[u8]>::from(damaged));

        let error = load_page_program_strict(&damaged, 0, PageContentLimits::default())
            .expect_err("a damaged chain is not strictly readable");
        assert!(error.kind().is_structural());

        let (program, repairs) = super::load_page_program_recovering(
            &damaged,
            0,
            PageContentLimits::default(),
            pdf_syntax::RecoverLimits::default(),
            b"",
        )
        .expect("the recovering loader rebuilds it")
        .into_parts();
        assert_eq!(program.streams.len(), 1);
        assert_eq!(program.streams[0].bytes.as_bytes(), b"0 0 1 1 re f");
        assert!(
            !repairs.is_empty(),
            "a rebuilt document reports what was rebuilt"
        );

        let (_, none) = super::load_page_program_recovering(
            &healthy,
            0,
            PageContentLimits::default(),
            pdf_syntax::RecoverLimits::default(),
            b"",
        )
        .expect("a healthy document")
        .into_parts();
        assert!(none.is_empty());

        let error = super::load_page_program_recovering(
            &healthy,
            7,
            PageContentLimits::default(),
            pdf_syntax::RecoverLimits::default(),
            b"",
        )
        .expect_err("page seven of a one-page document");
        assert_eq!(error.kind(), PageContentErrorKind::PageOutOfRange);
        assert!(!error.kind().is_structural());
    }

    fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
        haystack
            .windows(needle.len())
            .position(|window| window == needle)
    }
}
