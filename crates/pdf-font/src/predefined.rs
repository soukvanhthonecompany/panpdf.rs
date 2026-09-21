use std::sync::{Arc, OnceLock};

use pdf_bytes::{ByteStore, SourceId};

use crate::cmap::{CMap, CMapError, CMapLimits, CMapOrigin, parse_cmap_using};

const RESOURCE_SOURCE: SourceId = SourceId::new(u64::MAX);

struct Resource {
    name: &'static str,
    bytes: &'static [u8],
    cached: OnceLock<Result<Arc<CMap>, CMapError>>,
}

static RESOURCES: [Resource; 1] = [Resource {
    name: "UniKS-UTF16-H",
    bytes: include_bytes!("../resources/cmap/Adobe-Korea1/UniKS-UTF16-H"),
    cached: OnceLock::new(),
}];

#[must_use]
pub fn is_shipped(name: &[u8]) -> Option<&'static str> {
    RESOURCES
        .iter()
        .find(|resource| resource.name.as_bytes() == name)
        .map(|resource| resource.name)
}

pub fn load(name: &[u8]) -> Result<Arc<CMap>, CMapError> {
    let resource = RESOURCES
        .iter()
        .find(|resource| resource.name.as_bytes() == name)
        .ok_or(CMapError::UnknownPredefinedCMap)?;
    resource
        .cached
        .get_or_init(|| parse(resource, CMapLimits::default()))
        .clone()
}

fn parse(resource: &Resource, limits: CMapLimits) -> Result<Arc<CMap>, CMapError> {
    let source = ByteStore::new(RESOURCE_SOURCE, Arc::<[u8]>::from(resource.bytes));
    parse_cmap_using(
        &source,
        limits,
        CMapOrigin::Predefined(resource.name),
        &|used| resolve(used, limits),
    )
    .map(Arc::new)
}

fn resolve(name: &[u8], limits: CMapLimits) -> Option<Result<CMap, CMapError>> {
    let resource = RESOURCES
        .iter()
        .find(|resource| resource.name.as_bytes() == name)?;
    let deeper = CMapLimits {
        max_use_depth: limits.max_use_depth.saturating_sub(1),
        ..limits
    };
    Some(parse(resource, deeper).map(|used| (*used).clone()))
}

#[cfg(test)]
mod tests {
    use super::{is_shipped, load};
    use crate::cmap::CMapOrigin;

    const KNOWN: &[(u16, u32)] = &[
        (0x0020, 1),
        (0x00a0, 1),
        (0x4f88, 7336),
        (0x6606, 3777),
        (0x7d30, 5498),
        (0x9a5f, 7328),
        (0xb13b, 10327),
        (0xb6ec, 11509),
        (0xbc9d, 12644),
        (0xc24e, 13816),
        (0xc7ff, 14907),
        (0xcdb0, 2942),
        (0xd361, 17288),
    ];

    #[test]
    fn maps_the_codes_two_implementations_agree_on() {
        let cmap = load(b"UniKS-UTF16-H").expect("the shipped resource parses");
        assert_eq!(cmap.origin(), CMapOrigin::Predefined("UniKS-UTF16-H"));
        for (code, expected) in KNOWN {
            let bytes = code.to_be_bytes();
            let codes = cmap
                .source_codes(&bytes, |_| 0.0)
                .expect("the code decodes");
            assert_eq!(codes.len(), 1, "<{code:04x}> is one two-byte code");
            assert_eq!(
                codes[0].cid,
                Some(*expected),
                "<{code:04x}> selects CID {expected}"
            );
            assert!(codes[0].mapping_span.is_none());
        }
    }

    #[test]
    fn honours_the_declared_notdef_range() {
        let cmap = load(b"UniKS-UTF16-H").expect("the shipped resource parses");
        for code in [0x0000_u16, 0x0007, 0x001f] {
            let codes = cmap
                .source_codes(&code.to_be_bytes(), |_| 0.0)
                .expect("the code decodes");
            assert_eq!(codes[0].cid, Some(1), "<{code:04x}> is a declared notdef");
        }
        let codes = cmap
            .source_codes(&0x0086_u16.to_be_bytes(), |_| 0.0)
            .expect("the code decodes");
        assert_eq!(codes[0].cid, Some(0));
    }

    #[test]
    fn reads_the_four_byte_surrogate_code_space() {
        let cmap = load(b"UniKS-UTF16-H").expect("the shipped resource parses");
        let pair = cmap
            .source_codes(&[0xd8, 0x00, 0xdc, 0x00], |_| 0.0)
            .expect("a surrogate pair decodes");
        assert_eq!(pair.len(), 1);
        assert_eq!(pair[0].bytes.len(), 4);
        assert_eq!(pair[0].cid, Some(0));

        let stray = cmap
            .source_codes(&[0xd8, 0x00, 0x00, 0x41], |_| 0.0)
            .expect("a lone high surrogate keeps its declared width");
        assert_eq!(stray.len(), 1);
        assert_eq!(stray[0].bytes.len(), 4);
        assert_eq!(stray[0].cid, Some(0));
    }

    #[test]
    fn the_selected_cid_drives_the_advance_of_the_next_glyph() {
        use std::sync::Arc;

        use pdf_bytes::{ByteStore, SourceId};
        use pdf_syntax::{ObjectParser, ParseLimits};

        use crate::{CompositeFont, FontError, parse_cid_font};

        let bytes: &[u8] = b"<< /Subtype /CIDFontType2 /DW 1000 /W [0[0 313 278 355] 104 [222]] >>";
        let source = ByteStore::new(SourceId::new(3), Arc::<[u8]>::from(bytes));
        let object = ObjectParser::new(&source, 0, ParseLimits::default())
            .parse_next()
            .expect("the fixture parses")
            .expect("one object");
        let descendant = parse_cid_font(&source, &object, &|_| {
            Err(FontError::IndirectEntryUnresolved)
        })
        .expect("CID metrics");

        let cmap = load(b"UniKS-UTF16-H").expect("the shipped resource parses");
        let font = CompositeFont::new((*cmap).clone(), descendant.clone(), object.span());
        let codes = font
            .source_codes(&[0x00, 0x20, 0x00, 0xa0, 0x00, 0xb7])
            .expect("codes");
        assert_eq!(
            codes.iter().map(|code| code.cid).collect::<Vec<_>>(),
            vec![Some(1), Some(1), Some(104)]
        );
        assert_eq!(
            codes.iter().map(|code| code.width).collect::<Vec<_>>(),
            vec![313.0, 313.0, 222.0]
        );

        let identity = CompositeFont::new(
            crate::CMap::identity_horizontal(),
            descendant,
            object.span(),
        );
        let codes = identity
            .source_codes(&[0x00, 0x20, 0x00, 0xa0, 0x00, 0xb7])
            .expect("codes");
        assert_eq!(
            codes.iter().map(|code| code.cid).collect::<Vec<_>>(),
            vec![Some(0x20), Some(0xa0), Some(0xb7)]
        );
        assert_eq!(
            codes.iter().map(|code| code.width).collect::<Vec<_>>(),
            vec![1000.0, 1000.0, 1000.0]
        );
    }

    #[test]
    fn refuses_a_name_this_build_does_not_hold() {
        assert!(is_shipped(b"UniKS-UTF16-H").is_some());
        assert!(is_shipped(b"90ms-RKSJ-H").is_none());
        assert_eq!(
            load(b"90ms-RKSJ-H"),
            Err(crate::cmap::CMapError::UnknownPredefinedCMap)
        );
    }
}
