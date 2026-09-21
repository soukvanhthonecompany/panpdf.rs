use std::collections::{BTreeMap, HashMap};
use std::ops::ControlFlow;

use pdf_bytes::ByteStore;
use pdf_syntax::{NumberKind, Object, ObjectKind, RecoverLimits, Recovered, Reference, Repair};

use super::{
    PageContentError, PageContentErrorKind, PageContentLimits, PageTree, UnreadableAnnotation,
    dictionary, entry, find_page, open_page_tree, open_page_tree_recovering, walk_page_tree,
};

const MAX_LINKS: usize = 65_536;
const MAX_NAME_TREE_NODES: usize = 65_536;
const MAX_STRING_BYTES: usize = 1 << 20;

#[derive(Clone, Debug, PartialEq)]
pub struct Link {
    pub reference: Option<Reference>,
    pub rect: [f64; 4],
    pub quad_points: Vec<[f64; 8]>,
    pub flags: u32,
    pub action: Option<LinkAction>,
    pub destination: Option<Destination>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Followed<'a> {
    Uri(&'a [u8]),
    Page(&'a Destination),
    Other(&'a LinkAction),
    Nothing,
}

impl Link {
    #[must_use]
    pub fn followed(&self) -> Followed<'_> {
        match &self.action {
            Some(LinkAction::Uri(uri)) => Followed::Uri(uri),
            Some(LinkAction::GoTo(Some(destination))) => Followed::Page(destination),
            Some(LinkAction::GoTo(None)) => Followed::Nothing,
            Some(other) => Followed::Other(other),
            None => self
                .destination
                .as_ref()
                .map_or(Followed::Nothing, Followed::Page),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum LinkAction {
    Uri(Vec<u8>),
    GoTo(Option<Destination>),
    RemoteGoTo {
        file: Option<Vec<u8>>,
        destination: Option<Destination>,
    },
    EmbeddedGoTo {
        file: Option<Vec<u8>>,
        destination: Option<Destination>,
    },
    Launch {
        file: Option<Vec<u8>>,
    },
    Named(Vec<u8>),
    Other(Vec<u8>),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Destination {
    pub page: Option<usize>,
    pub view: View,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum View {
    Xyz {
        left: Option<f64>,
        top: Option<f64>,
        zoom: Option<f64>,
    },
    Fit,
    FitH {
        top: Option<f64>,
    },
    FitV {
        left: Option<f64>,
    },
    FitR {
        rect: [f64; 4],
    },
    FitB,
    FitBH {
        top: Option<f64>,
    },
    FitBV {
        left: Option<f64>,
    },
    Unknown,
}

#[derive(Clone, Debug, Default)]
pub struct PageLinks {
    pub links: Vec<Link>,
    pub unreadable: Vec<UnreadableAnnotation>,
}

pub struct LinkResolver {
    source: ByteStore,
    tree: PageTree,
    pages: HashMap<Reference, usize>,
    page_count: usize,
    limits: PageContentLimits,
}

impl std::fmt::Debug for LinkResolver {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("LinkResolver")
            .field("page_count", &self.page_count)
            .finish_non_exhaustive()
    }
}

pub fn open_link_resolver(
    source: &ByteStore,
    limits: PageContentLimits,
    password: &[u8],
) -> Result<LinkResolver, PageContentError> {
    LinkResolver::new(source, open_page_tree(source, limits, password)?, limits)
}

pub fn open_link_resolver_recovering(
    source: &ByteStore,
    limits: PageContentLimits,
    recover: RecoverLimits,
    password: &[u8],
) -> Result<Recovered<LinkResolver>, PageContentError> {
    match open_link_resolver(source, limits, password) {
        Ok(resolver) => return Ok(Recovered::new(resolver, Vec::new())),
        Err(error) if !error.kind().is_structural() => return Err(error),
        Err(_) => {}
    }
    let (tree, mut repairs) =
        open_page_tree_recovering(source, limits, recover, password)?.into_parts();
    let resolver = LinkResolver::new(source, tree.tolerating_damage(), limits)?;
    repairs.push(Repair::tolerated_structural_damage(0));
    Ok(Recovered::new(resolver, repairs))
}

pub fn open_link_resolver_tolerating_damage(
    source: &ByteStore,
    limits: PageContentLimits,
    recover: RecoverLimits,
    password: &[u8],
) -> Result<Recovered<LinkResolver>, PageContentError> {
    let (tree, mut repairs) =
        open_page_tree_recovering(source, limits, recover, password)?.into_parts();
    let resolver = LinkResolver::new(source, tree.tolerating_damage(), limits)?;
    repairs.push(Repair::tolerated_structural_damage(0));
    Ok(Recovered::new(resolver, repairs))
}

#[derive(Clone)]
struct Found {
    source: ByteStore,
    value: Object,
    strings: Strings,
}

#[derive(Clone, Copy)]
enum Strings {
    Plain,
    EncryptedWith(Reference),
}

impl LinkResolver {
    fn new(
        source: &ByteStore,
        tree: PageTree,
        limits: PageContentLimits,
    ) -> Result<Self, PageContentError> {
        let mut pages = HashMap::new();
        let mut page_count = 0_usize;
        walk_page_tree(
            source,
            &tree.index,
            tree.root,
            limits,
            (),
            |_, ()| Ok(()),
            |reference, ()| {
                pages.entry(reference).or_insert(page_count);
                page_count += 1;
                Ok(ControlFlow::<()>::Continue(()))
            },
        )?;
        Ok(Self {
            source: source.clone(),
            tree,
            pages,
            page_count,
            limits,
        })
    }

    #[must_use]
    pub const fn page_count(&self) -> usize {
        self.page_count
    }

    pub fn links(&self, page_index: usize) -> Result<PageLinks, PageContentError> {
        let found = find_page(
            &self.source,
            &self.tree.index,
            self.tree.root,
            page_index,
            self.limits,
        )?;
        let page = self.resolve(found.reference)?;
        let entries = dictionary(&page.value)
            .ok_or_else(|| PageContentError::new(PageContentErrorKind::PageNotDictionary))?;
        let mut out = PageLinks::default();
        let Some(annots) = entry(entries, &page.source, b"/Annots") else {
            return Ok(out);
        };
        let annots = match self.follow(&page, annots) {
            Ok(annots) => annots,
            Err(error) => {
                out.unreadable.push(UnreadableAnnotation {
                    reference: None,
                    error,
                });
                return Ok(out);
            }
        };
        let ObjectKind::Array(items) = annots.value.kind() else {
            return Ok(out);
        };
        for item in items.iter().take(MAX_LINKS) {
            let reference = match item.kind() {
                ObjectKind::Reference(reference) => Some(*reference),
                _ => None,
            };
            match self.link(&annots, item, reference) {
                Ok(Some(link)) => out.links.push(link),
                Ok(None) => {}
                Err(error) => out
                    .unreadable
                    .push(UnreadableAnnotation { reference, error }),
            }
        }
        if items.len() > MAX_LINKS {
            out.unreadable.push(UnreadableAnnotation {
                reference: None,
                error: PageContentError::new(PageContentErrorKind::AnnotationLimit),
            });
        }
        Ok(out)
    }

    fn link(
        &self,
        list: &Found,
        item: &Object,
        reference: Option<Reference>,
    ) -> Result<Option<Link>, PageContentError> {
        let annotation = self.follow(list, item)?;
        let Some(entries) = dictionary(&annotation.value) else {
            if matches!(annotation.value.kind(), ObjectKind::Null) {
                return Ok(None);
            }
            return Err(PageContentError::new(
                PageContentErrorKind::AnnotationNotDictionary,
            ));
        };
        let source = &annotation.source;
        if !entry(entries, source, b"/Subtype")
            .is_some_and(|subtype| subtype.name_equals(source, b"/Link"))
        {
            return Ok(None);
        }
        let rect = entry(entries, source, b"/Rect")
            .and_then(|value| self.numbers(&annotation, value))
            .and_then(|numbers| <[f64; 4]>::try_from(numbers.as_slice()).ok())
            .map(|[x0, y0, x1, y1]| [x0.min(x1), y0.min(y1), x0.max(x1), y0.max(y1)])
            .ok_or_else(|| PageContentError::new(PageContentErrorKind::InvalidAnnotationRect))?;
        let quad_points = entry(entries, source, b"/QuadPoints")
            .and_then(|value| self.numbers(&annotation, value))
            .map(|numbers| {
                numbers
                    .chunks_exact(8)
                    .filter_map(|quad| <[f64; 8]>::try_from(quad).ok())
                    .collect()
            })
            .unwrap_or_default();
        let flags = entry(entries, source, b"/F")
            .and_then(|value| self.follow(&annotation, value).ok())
            .and_then(|found| integer(&found.source, &found.value))
            .and_then(|value| u32::try_from(value).ok())
            .unwrap_or(0);
        let action =
            entry(entries, source, b"/A").and_then(|value| self.action(&annotation, value));
        let destination = entry(entries, source, b"/Dest")
            .and_then(|value| self.destination(&annotation, value, true));
        Ok(Some(Link {
            reference,
            rect,
            quad_points,
            flags,
            action,
            destination,
        }))
    }

    fn action(&self, from: &Found, value: &Object) -> Option<LinkAction> {
        let action = self.follow(from, value).ok()?;
        let entries = dictionary(&action.value)?;
        let source = &action.source;
        let kind = entry(entries, source, b"/S").and_then(|value| name(source, value))?;
        let remote_destination = || {
            entry(entries, source, b"/D").and_then(|value| self.destination(&action, value, false))
        };
        Some(match kind.as_slice() {
            b"URI" => LinkAction::Uri(
                entry(entries, source, b"/URI")
                    .and_then(|value| self.string(&action, value))
                    .map(|uri| self.with_base(uri))
                    .unwrap_or_default(),
            ),
            b"GoTo" => LinkAction::GoTo(
                entry(entries, source, b"/D")
                    .and_then(|value| self.destination(&action, value, true)),
            ),
            b"GoToR" => LinkAction::RemoteGoTo {
                file: self.file(&action, entries),
                destination: remote_destination(),
            },
            b"GoToE" => LinkAction::EmbeddedGoTo {
                file: self.file(&action, entries),
                destination: remote_destination(),
            },
            b"Launch" => LinkAction::Launch {
                file: self.file(&action, entries),
            },
            b"Named" => LinkAction::Named(
                entry(entries, source, b"/N")
                    .and_then(|value| name(source, value))
                    .unwrap_or_default(),
            ),
            _ => LinkAction::Other(kind),
        })
    }

    fn file(&self, action: &Found, entries: &[pdf_syntax::DictionaryEntry]) -> Option<Vec<u8>> {
        let value = entry(entries, &action.source, b"/F")?;
        let found = self.follow(action, value).ok()?;
        if let Some(specification) = dictionary(&found.value) {
            return [b"/UF".as_slice(), b"/F"].iter().find_map(|key| {
                entry(specification, &found.source, key)
                    .and_then(|value| self.string(&found, value))
            });
        }
        self.string(&found, &found.value)
    }

    fn with_base(&self, uri: Vec<u8>) -> Vec<u8> {
        if has_scheme(&uri) {
            return uri;
        }
        let base = self.catalog().and_then(|catalog| {
            let entries = dictionary(&catalog.value)?;
            let value = entry(entries, &catalog.source, b"/URI")?;
            let dictionary_found = self.follow(&catalog, value).ok()?;
            let uri_entries = dictionary(&dictionary_found.value)?;
            let base = entry(uri_entries, &dictionary_found.source, b"/Base")?;
            self.string(&dictionary_found, base)
        });
        match base {
            Some(mut base) => {
                base.extend_from_slice(&uri);
                base
            }
            None => uri,
        }
    }

    fn destination(&self, from: &Found, value: &Object, local: bool) -> Option<Destination> {
        let found = self.follow(from, value).ok()?;
        match found.value.kind() {
            ObjectKind::Array(_) => self.explicit(&found, local),
            ObjectKind::Dictionary(entries) => {
                let inner = entry(entries, &found.source, b"/D")?;
                let inner = self.follow(&found, inner).ok()?;
                self.explicit(&inner, local)
            }
            ObjectKind::Name if local => {
                let key = name(&found.source, &found.value)?;
                self.named(&key)
            }
            ObjectKind::LiteralString | ObjectKind::HexString if local => {
                let key = self.string(&found, &found.value)?;
                self.named(&key)
            }
            _ => None,
        }
    }

    fn named(&self, key: &[u8]) -> Option<Destination> {
        let catalog = self.catalog()?;
        let entries = dictionary(&catalog.value)?;
        let from_tree = entry(entries, &catalog.source, b"/Names")
            .and_then(|value| self.follow(&catalog, value).ok())
            .and_then(|names| {
                let dests = entry(dictionary(&names.value)?, &names.source, b"/Dests")?;
                let root = self.follow(&names, dests).ok()?;
                self.name_tree(root, key)
            });
        let found = from_tree.or_else(|| {
            let dests = entry(entries, &catalog.source, b"/Dests")?;
            let dests = self.follow(&catalog, dests).ok()?;
            let mut solidus = Vec::with_capacity(key.len() + 1);
            solidus.push(b'/');
            solidus.extend_from_slice(key);
            let value = entry(dictionary(&dests.value)?, &dests.source, &solidus)?;
            self.follow(&dests, value).ok()
        })?;
        self.destination_at(&found)
    }

    fn destination_at(&self, found: &Found) -> Option<Destination> {
        match found.value.kind() {
            ObjectKind::Array(_) => self.explicit(found, true),
            ObjectKind::Dictionary(entries) => {
                let inner = entry(entries, &found.source, b"/D")?;
                let inner = self.follow(found, inner).ok()?;
                self.explicit(&inner, true)
            }
            _ => None,
        }
    }

    #[must_use]
    pub fn destination_named(&self, name: &[u8]) -> Option<Destination> {
        self.named(name)
    }

    #[must_use]
    pub fn destinations(&self) -> Vec<(Vec<u8>, Destination)> {
        let mut found: BTreeMap<Vec<u8>, Destination> = BTreeMap::new();
        let Some(catalog) = self.catalog() else {
            return Vec::new();
        };
        let Some(entries) = dictionary(&catalog.value) else {
            return Vec::new();
        };
        if let Some(names) = entry(entries, &catalog.source, b"/Names")
            .and_then(|value| self.follow(&catalog, value).ok())
            && let Some(dests) = dictionary(&names.value)
                .and_then(|entries| entry(entries, &names.source, b"/Dests"))
            && let Ok(root) = self.follow(&names, dests)
        {
            self.walk_name_tree(root, &mut found);
        }
        if let Some(dests) = entry(entries, &catalog.source, b"/Dests")
            .and_then(|value| self.follow(&catalog, value).ok())
            && let Some(pairs) = dictionary(&dests.value)
        {
            for pair in pairs.iter().take(MAX_NAME_TREE_NODES) {
                let Ok(mut key) = pair.decoded_key(&dests.source) else {
                    continue;
                };
                if key.first() == Some(&b'/') {
                    key.remove(0);
                }
                if found.contains_key(&key) || key.len() > MAX_STRING_BYTES {
                    continue;
                }
                if let Ok(value) = self.follow(&dests, pair.value())
                    && let Some(destination) = self.destination_at(&value)
                {
                    found.insert(key, destination);
                }
            }
        }
        found.into_iter().collect()
    }

    fn walk_name_tree(&self, root: Found, out: &mut BTreeMap<Vec<u8>, Destination>) {
        let mut pending = vec![root];
        let mut visited = 0_usize;
        while let Some(node) = pending.pop() {
            visited += 1;
            if visited > MAX_NAME_TREE_NODES {
                return;
            }
            let Some(entries) = dictionary(&node.value) else {
                continue;
            };
            if let Some(names) = entry(entries, &node.source, b"/Names")
                .and_then(|value| self.follow(&node, value).ok())
                && let ObjectKind::Array(pairs) = names.value.kind()
            {
                for pair in pairs.chunks_exact(2) {
                    let Some(key) = self
                        .string(&names, &pair[0])
                        .or_else(|| name(&names.source, &pair[0]))
                    else {
                        continue;
                    };
                    if out.contains_key(&key) {
                        continue;
                    }
                    if let Ok(value) = self.follow(&names, &pair[1])
                        && let Some(destination) = self.destination_at(&value)
                    {
                        out.insert(key, destination);
                    }
                }
            }
            if let Some(kids) = entry(entries, &node.source, b"/Kids")
                .and_then(|value| self.follow(&node, value).ok())
                && let ObjectKind::Array(children) = kids.value.kind()
            {
                pending.extend(
                    children
                        .iter()
                        .rev()
                        .filter_map(|child| self.follow(&kids, child).ok()),
                );
            }
        }
    }

    fn name_tree(&self, root: Found, key: &[u8]) -> Option<Found> {
        let mut pending = vec![root];
        let mut visited = 0_usize;
        while let Some(node) = pending.pop() {
            visited += 1;
            if visited > MAX_NAME_TREE_NODES {
                return None;
            }
            let Some(entries) = dictionary(&node.value) else {
                continue;
            };
            if let Some([low, high]) = entry(entries, &node.source, b"/Limits")
                .and_then(|value| self.follow(&node, value).ok())
                .and_then(|limits| match limits.value.kind() {
                    ObjectKind::Array(bounds) if bounds.len() == 2 => Some([
                        self.string(&limits, &bounds[0])?,
                        self.string(&limits, &bounds[1])?,
                    ]),
                    _ => None,
                })
                && (key < low.as_slice() || key > high.as_slice())
            {
                continue;
            }
            if let Some(names) = entry(entries, &node.source, b"/Names")
                .and_then(|value| self.follow(&node, value).ok())
                && let ObjectKind::Array(pairs) = names.value.kind()
            {
                for pair in pairs.chunks_exact(2) {
                    let Some(written) = self
                        .string(&names, &pair[0])
                        .or_else(|| name(&names.source, &pair[0]))
                    else {
                        continue;
                    };
                    if written.as_slice() == key {
                        return self.follow(&names, &pair[1]).ok();
                    }
                    if written.as_slice() > key {
                        break;
                    }
                }
            }
            if let Some(kids) = entry(entries, &node.source, b"/Kids")
                .and_then(|value| self.follow(&node, value).ok())
                && let ObjectKind::Array(children) = kids.value.kind()
            {
                pending.extend(
                    children
                        .iter()
                        .rev()
                        .filter_map(|child| self.follow(&kids, child).ok()),
                );
            }
        }
        None
    }

    fn explicit(&self, found: &Found, local: bool) -> Option<Destination> {
        let ObjectKind::Array(items) = found.value.kind() else {
            return None;
        };
        let first = items.first()?;
        let page = match first.kind() {
            ObjectKind::Reference(reference) if local => self.pages.get(reference).copied(),
            ObjectKind::Number(NumberKind::Integer) => {
                integer(&found.source, first).and_then(|index| usize::try_from(index).ok())
            }
            _ => None,
        };
        let coordinate = |index: usize| -> Option<f64> {
            items
                .get(index)
                .and_then(|item| number(&found.source, item))
        };
        let view = match items
            .get(1)
            .and_then(|item| name(&found.source, item))
            .as_deref()
        {
            Some(b"XYZ") => View::Xyz {
                left: coordinate(2),
                top: coordinate(3),
                zoom: coordinate(4).filter(|zoom| *zoom != 0.0),
            },
            Some(b"Fit") => View::Fit,
            Some(b"FitB") => View::FitB,
            Some(b"FitH") => View::FitH { top: coordinate(2) },
            Some(b"FitBH") => View::FitBH { top: coordinate(2) },
            Some(b"FitV") => View::FitV {
                left: coordinate(2),
            },
            Some(b"FitBV") => View::FitBV {
                left: coordinate(2),
            },
            Some(b"FitR") => match (coordinate(2), coordinate(3), coordinate(4), coordinate(5)) {
                (Some(x0), Some(y0), Some(x1), Some(y1)) => View::FitR {
                    rect: [x0, y0, x1, y1],
                },
                _ => View::Unknown,
            },
            _ => View::Unknown,
        };
        Some(Destination { page, view })
    }

    fn numbers(&self, from: &Found, value: &Object) -> Option<Vec<f64>> {
        let found = self.follow(from, value).ok()?;
        let ObjectKind::Array(items) = found.value.kind() else {
            return None;
        };
        items
            .iter()
            .map(|item| {
                let item = self.follow(&found, item).ok()?;
                number(&item.source, &item.value)
            })
            .collect()
    }

    fn string(&self, holder: &Found, value: &Object) -> Option<Vec<u8>> {
        let found = self.follow(holder, value).ok()?;
        let bytes =
            pdf_syntax::decode_string(&found.source, &found.value, MAX_STRING_BYTES).ok()?;
        match (found.strings, self.tree.security.as_ref()) {
            (Strings::EncryptedWith(reference), Some(security)) => {
                security.decrypt_string(reference, &bytes).ok()
            }
            _ => Some(bytes),
        }
    }

    fn catalog(&self) -> Option<Found> {
        self.resolve(self.tree.catalog).ok()
    }

    fn resolve(&self, reference: Reference) -> Result<Found, PageContentError> {
        let resolved = self
            .tree
            .index
            .resolve_object(&self.source, reference, self.limits.resolve)
            .map_err(|error| PageContentError::new(PageContentErrorKind::Resolve(error)))?;
        let strings = if self.tree.security.is_none() || resolved.is_compressed() {
            Strings::Plain
        } else {
            Strings::EncryptedWith(reference)
        };
        Ok(Found {
            source: resolved.source().clone(),
            value: resolved.value().clone(),
            strings,
        })
    }

    fn follow(&self, from: &Found, value: &Object) -> Result<Found, PageContentError> {
        match value.kind() {
            ObjectKind::Reference(reference) => self.resolve(*reference),
            _ => Ok(Found {
                source: from.source.clone(),
                value: value.clone(),
                strings: from.strings,
            }),
        }
    }
}

fn name(source: &ByteStore, value: &Object) -> Option<Vec<u8>> {
    let mut decoded = pdf_syntax::decode_name(source, value).ok()?;
    if decoded.first() == Some(&b'/') {
        decoded.remove(0);
    }
    Some(decoded)
}

fn number(source: &ByteStore, value: &Object) -> Option<f64> {
    if !matches!(value.kind(), ObjectKind::Number(_)) {
        return None;
    }
    std::str::from_utf8(source.resolve(value.span()).ok()?)
        .ok()?
        .parse::<f64>()
        .ok()
        .filter(|number| number.is_finite())
}

fn integer(source: &ByteStore, value: &Object) -> Option<i64> {
    if !matches!(value.kind(), ObjectKind::Number(NumberKind::Integer)) {
        return None;
    }
    std::str::from_utf8(source.resolve(value.span()).ok()?)
        .ok()?
        .parse::<i64>()
        .ok()
}

fn has_scheme(uri: &[u8]) -> bool {
    let Some(colon) = uri.iter().position(|byte| *byte == b':') else {
        return false;
    };
    let scheme = &uri[..colon];
    scheme.first().is_some_and(u8::is_ascii_alphabetic)
        && scheme
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'-' | b'.'))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use pdf_bytes::{ByteStore, SourceId};

    use super::{Destination, LinkAction, View, has_scheme, open_link_resolver};
    use crate::page::PageContentLimits;

    fn document(catalog: &[u8], link: &[u8], extra: &[&[u8]]) -> ByteStore {
        let mut objects: Vec<Vec<u8>> = vec![
            [
                b"<< /Type /Catalog /Pages 2 0 R ".as_slice(),
                catalog,
                b" >>",
            ]
            .concat(),
            b"<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 /MediaBox [0 0 200 200] >>".to_vec(),
            b"<< /Type /Page /Parent 2 0 R /Annots [5 0 R] >>".to_vec(),
            b"<< /Type /Page /Parent 2 0 R >>".to_vec(),
            [
                b"<< /Type /Annot /Subtype /Link ".as_slice(),
                if link.windows(5).any(|window| window == b"/Rect") {
                    b"".as_slice()
                } else {
                    b"/Rect [10 20 110 40] "
                },
                link,
                b" >>",
            ]
            .concat(),
        ];
        objects.extend(extra.iter().map(|body| body.to_vec()));
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
        for offset in offsets {
            bytes.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
        }
        bytes.extend_from_slice(
            format!(
                "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
                objects.len() + 1
            )
            .as_bytes(),
        );
        ByteStore::new(SourceId::new(71), Arc::<[u8]>::from(bytes))
    }

    fn only_link(catalog: &[u8], link: &[u8], extra: &[&[u8]]) -> super::Link {
        let resolver = open_link_resolver(
            &document(catalog, link, extra),
            PageContentLimits::default(),
            b"",
        )
        .expect("the document opens");
        let mut found = resolver.links(0).expect("page one reads");
        assert!(found.unreadable.is_empty(), "{:?}", found.unreadable);
        assert_eq!(found.links.len(), 1);
        found.links.remove(0)
    }

    const FIT_PAGE_TWO: Destination = Destination {
        page: Some(1),
        view: View::Fit,
    };

    #[test]
    fn a_uri_action_is_read_and_a_relative_one_takes_the_base() {
        let absolute = only_link(b"", b"/A << /S /URI /URI (https://example.com/a) >>", &[]);
        assert_eq!(
            absolute.action,
            Some(LinkAction::Uri(b"https://example.com/a".to_vec()))
        );
        let relative = only_link(
            b"/URI << /Base (https://example.com/a/) >>",
            b"/A << /S /URI /URI (b/c.html) >>",
            &[],
        );
        assert_eq!(
            relative.action,
            Some(LinkAction::Uri(b"https://example.com/a/b/c.html".to_vec()))
        );
        let untouched = only_link(
            b"/URI << /Base (https://example.com/a/) >>",
            b"/A << /S /URI /URI (https://other.example/x) >>",
            &[],
        );
        assert_eq!(
            untouched.action,
            Some(LinkAction::Uri(b"https://other.example/x".to_vec()))
        );
    }

    #[test]
    fn an_explicit_destination_names_its_page_by_position() {
        let link = only_link(b"", b"/Dest [4 0 R /XYZ 10 150 0]", &[]);
        assert_eq!(
            link.destination,
            Some(Destination {
                page: Some(1),
                view: View::Xyz {
                    left: Some(10.0),
                    top: Some(150.0),
                    zoom: None,
                },
            })
        );
    }

    #[test]
    fn a_page_outside_the_tree_lands_nowhere_and_a_number_is_an_index() {
        let outside = only_link(b"", b"/Dest [6 0 R /Fit]", &[b"<< /Type /Page >>"]);
        assert_eq!(
            outside.destination,
            Some(Destination {
                page: None,
                view: View::Fit,
            })
        );
        let numbered = only_link(b"", b"/Dest [5 /Fit]", &[]);
        assert_eq!(numbered.destination.and_then(|d| d.page), Some(5));
    }

    #[test]
    fn a_named_destination_is_looked_up_in_the_tree_before_the_dictionary() {
        let link = only_link(
            b"/Dests 6 0 R /Names << /Dests 7 0 R >>",
            b"/Dest /chapter",
            &[
                b"<< /chapter [3 0 R /Fit] >>",
                b"<< /Kids [8 0 R] >>",
                b"<< /Limits [(a) (z)] /Names [(chapter) [4 0 R /Fit]] >>",
            ],
        );
        assert_eq!(link.destination, Some(FIT_PAGE_TWO));
        let dictionary_only = only_link(
            b"/Dests 6 0 R",
            b"/Dest (chapter)",
            &[b"<< /chapter [4 0 R /Fit] >>"],
        );
        assert_eq!(dictionary_only.destination, Some(FIT_PAGE_TWO));
    }

    #[test]
    fn a_rect_of_indirect_numbers_is_read_through_them() {
        let link = only_link(
            b"",
            b"/Rect [6 0 R 7 0 R 8 0 R 9 0 R] /A << /S /GoTo /D [4 0 R /Fit] >> \
              /QuadPoints [10 40 60 40 10 20 60 20]",
            &[b"110", b"40", b"10", b"20"],
        );
        assert!(
            link.rect
                .iter()
                .zip([10.0, 20.0, 110.0, 40.0])
                .all(|(read, expected)| (read - expected).abs() < 1e-9),
            "{:?}",
            link.rect
        );
        assert_eq!(link.action, Some(LinkAction::GoTo(Some(FIT_PAGE_TWO))));
        assert_eq!(link.quad_points.len(), 1);
    }

    #[test]
    fn a_name_tree_leaf_is_searched_in_order_and_stops_at_a_larger_key() {
        let sorted = only_link(
            b"/Names << /Dests 6 0 R >>",
            b"/Dest (chapter)",
            &[b"<< /Names [(a) [3 0 R /Fit] (chapter) [4 0 R /Fit]] >>"],
        );
        assert_eq!(sorted.destination, Some(FIT_PAGE_TWO));
        let unsorted = only_link(
            b"/Names << /Dests 6 0 R >>",
            b"/Dest (chapter)",
            &[b"<< /Names [(z) [3 0 R /Fit] (chapter) [4 0 R /Fit] (zz) [3 0 R /Fit]] >>"],
        );
        assert_eq!(unsorted.destination, None);
    }

    #[test]
    fn every_named_destination_is_listed_in_order() {
        let resolver = open_link_resolver(
            &document(
                b"/Dests 6 0 R /Names << /Dests 7 0 R >>",
                b"/Dest /chapter",
                &[
                    b"<< /chapter [3 0 R /Fit] /older [3 0 R /Fit] /broken (not a destination) >>",
                    b"<< /Kids [8 0 R 9 0 R] >>",
                    b"<< /Limits [(a) (zz)] /Names [(chapter) [4 0 R /Fit] (zed) [3 0 R /FitH 90]] >>",
                    b"<< /Limits [(a) (zz)] /Names [(chapter) [5 0 R /Fit]] >>",
                ],
            ),
            PageContentLimits::default(),
            b"",
        )
        .expect("the document opens");
        let listed = resolver.destinations();
        let names: Vec<&[u8]> = listed.iter().map(|(name, _)| name.as_slice()).collect();
        assert_eq!(names, vec![b"chapter".as_slice(), b"older", b"zed"]);
        assert_eq!(listed[0].1, FIT_PAGE_TWO);
        assert_eq!(listed[1].1.page, Some(0));
        assert_eq!(listed[2].1.view, View::FitH { top: Some(90.0) });
        for (name, _) in &listed {
            assert_eq!(
                resolver.destination_named(name).as_ref(),
                listed.iter().find(|(key, _)| key == name).map(|(_, d)| d),
                "{}",
                String::from_utf8_lossy(name)
            );
        }
    }

    #[test]
    fn a_document_that_names_no_destination_lists_none() {
        let resolver = open_link_resolver(
            &document(b"", b"/Dest [4 0 R /Fit]", &[]),
            PageContentLimits::default(),
            b"",
        )
        .expect("the document opens");
        assert!(resolver.destinations().is_empty());
        assert_eq!(resolver.destination_named(b"chapter"), None);
    }

    #[test]
    fn a_name_tree_key_written_as_a_name_still_matches() {
        let link = only_link(
            b"/Names << /Dests 6 0 R >>",
            b"/Dest /top",
            &[b"<< /Names [(other) [3 0 R /Fit] /top [4 0 R /Fit]] >>"],
        );
        assert_eq!(link.destination, Some(FIT_PAGE_TWO));
    }

    #[test]
    fn a_scheme_is_letters_before_a_colon() {
        assert!(has_scheme(b"https://example.com"));
        assert!(has_scheme(b"mailto:someone@example.com"));
        assert!(has_scheme(b"x-custom+1.0:thing"));
    }

    #[test]
    fn a_relative_reference_has_no_scheme() {
        assert!(!has_scheme(b"b/c.html"));
        assert!(!has_scheme(b"docs/a:b.html"));
        assert!(!has_scheme(b":nothing"));
        assert!(!has_scheme(b"1http://example.com"));
    }
}
