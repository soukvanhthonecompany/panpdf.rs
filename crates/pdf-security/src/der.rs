const MOST_NESTING: usize = 64;

pub(crate) mod tag {
    pub(crate) const BOOLEAN: u8 = 0x01;
    pub(crate) const INTEGER: u8 = 0x02;
    pub(crate) const BIT_STRING: u8 = 0x03;
    pub(crate) const OCTET_STRING: u8 = 0x04;
    pub(crate) const NULL: u8 = 0x05;
    pub(crate) const OID: u8 = 0x06;
    pub(crate) const UTC_TIME: u8 = 0x17;
    pub(crate) const GENERALIZED_TIME: u8 = 0x18;
    pub(crate) const SEQUENCE: u8 = 0x30;
    pub(crate) const SET: u8 = 0x31;
    pub(crate) const CONTEXT_0: u8 = 0xa0;
    pub(crate) const CONTEXT_1: u8 = 0xa1;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct DerError(&'static str);

impl std::fmt::Display for DerError {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.write_str(self.0)
    }
}

fn refused(reason: &'static str) -> DerError {
    DerError(reason)
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Element<'a> {
    pub(crate) tag: u8,
    pub(crate) content: &'a [u8],
    pub(crate) whole: &'a [u8],
    pub(crate) unstated_length: bool,
}

impl<'a> Element<'a> {
    pub(crate) fn reader(&self) -> Reader<'a> {
        Reader {
            rest: self.content,
            depth: 0,
        }
    }

    pub(crate) fn first(&self) -> Result<Element<'a>, DerError> {
        self.reader().next()
    }

    pub(crate) fn unsigned(&self) -> Result<&'a [u8], DerError> {
        if self.tag != tag::INTEGER {
            return Err(refused("an integer was expected"));
        }
        let mut octets = self.content;
        if octets.is_empty() {
            return Err(refused("an integer with no octets"));
        }
        while octets.len() > 1 && octets[0] == 0 {
            octets = &octets[1..];
        }
        Ok(octets)
    }

    pub(crate) fn bits(&self) -> Result<&'a [u8], DerError> {
        if self.tag != tag::BIT_STRING {
            return Err(refused("a bit string was expected"));
        }
        match self.content.split_first() {
            Some((0, bits)) => Ok(bits),
            Some(_) => Err(refused("a bit string that does not end on an octet")),
            None => Err(refused("a bit string with no octets at all")),
        }
    }

    pub(crate) fn oid(&self) -> Result<&'a [u8], DerError> {
        if self.tag != tag::OID {
            return Err(refused("an object identifier was expected"));
        }
        Ok(self.content)
    }
}

#[derive(Clone, Debug)]
pub(crate) struct Reader<'a> {
    rest: &'a [u8],
    depth: usize,
}

impl<'a> Reader<'a> {
    pub(crate) const fn new(bytes: &'a [u8]) -> Self {
        Self {
            rest: bytes,
            depth: 0,
        }
    }

    #[cfg(test)]
    pub(crate) const fn is_empty(&self) -> bool {
        self.rest.is_empty()
    }

    pub(crate) fn next(&mut self) -> Result<Element<'a>, DerError> {
        let (element, rest) = read(self.rest, self.depth)?;
        self.rest = rest;
        Ok(element)
    }

    pub(crate) fn expect(&mut self, tag: u8) -> Result<Element<'a>, DerError> {
        let element = self.next()?;
        if element.tag == tag {
            Ok(element)
        } else {
            Err(refused("a different element than the structure calls for"))
        }
    }

    pub(crate) fn optional(&mut self, tag: u8) -> Option<Element<'a>> {
        let (element, rest) = read(self.rest, self.depth).ok()?;
        if element.tag != tag {
            return None;
        }
        self.rest = rest;
        Some(element)
    }

    #[cfg(test)]
    pub(crate) fn peek(&self) -> Option<u8> {
        read(self.rest, self.depth)
            .ok()
            .map(|(element, _)| element.tag)
    }
}

fn read(bytes: &[u8], depth: usize) -> Result<(Element<'_>, &[u8]), DerError> {
    if depth > MOST_NESTING {
        return Err(refused("a structure nested deeper than this reads"));
    }
    let (&tag, after_tag) = bytes
        .split_first()
        .ok_or_else(|| refused("an element with no identifier"))?;
    if tag & 0x1f == 0x1f {
        return Err(refused("an identifier this does not read"));
    }
    let (&first, after_first) = after_tag
        .split_first()
        .ok_or_else(|| refused("an element with no length"))?;

    if first == 0x80 {
        if tag & 0x20 == 0 {
            return Err(refused("a primitive element with no length"));
        }
        let inside = span_to_end(after_first, depth + 1)?;
        let content = &after_first[..inside];
        let used = 2 + inside + 2;
        return Ok((
            Element {
                tag,
                content,
                whole: &bytes[..used],
                unstated_length: true,
            },
            &bytes[used..],
        ));
    }

    let (length, after_length) = if first & 0x80 == 0 {
        (usize::from(first), after_first)
    } else {
        let count = usize::from(first & 0x7f);
        if count > std::mem::size_of::<usize>() {
            return Err(refused("a length larger than this machine counts to"));
        }
        if after_first.len() < count {
            return Err(refused("a length that runs off the end"));
        }
        let (octets, rest) = after_first.split_at(count);
        let mut length = 0usize;
        for &octet in octets {
            length = (length << 8) | usize::from(octet);
        }
        (length, rest)
    };
    if after_length.len() < length {
        return Err(refused("an element that runs off the end"));
    }
    let content = &after_length[..length];
    let used = bytes.len() - after_length.len() + length;
    Ok((
        Element {
            tag,
            content,
            whole: &bytes[..used],
            unstated_length: false,
        },
        &bytes[used..],
    ))
}

fn span_to_end(bytes: &[u8], depth: usize) -> Result<usize, DerError> {
    let mut rest = bytes;
    loop {
        match rest.first() {
            None => return Err(refused("an element that never ends")),
            Some(0) => {
                if rest.get(1) == Some(&0) {
                    return Ok(bytes.len() - rest.len());
                }
                return Err(refused("an element ended by something that is not an end"));
            }
            Some(_) => {
                let (_, after) = read(rest, depth)?;
                rest = after;
            }
        }
    }
}

pub(crate) fn retagged(tag: u8, content: &[u8]) -> Vec<u8> {
    let mut out = vec![tag];
    let length = content.len();
    if length < 0x80 {
        #[expect(
            clippy::cast_possible_truncation,
            reason = "below 0x80 by the branch above"
        )]
        out.push(length as u8);
    } else {
        let octets = length.to_be_bytes();
        let first = octets.iter().position(|&octet| octet != 0).unwrap_or(0);
        let octets = &octets[first..];
        #[expect(
            clippy::cast_possible_truncation,
            reason = "a length is at most eight octets long"
        )]
        out.push(0x80 | octets.len() as u8);
        out.extend_from_slice(octets);
    }
    out.extend_from_slice(content);
    out
}

#[cfg(test)]
mod tests;
