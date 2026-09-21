pub mod tag {
    pub const OPERATION: u8 = 0x01;
    pub const JOB: u8 = 0x02;
    pub const END: u8 = 0x03;
    pub const PRINTER: u8 = 0x04;
    pub const UNSUPPORTED_GROUP: u8 = 0x05;
    pub const UNSUPPORTED: u8 = 0x10;
    pub const UNKNOWN: u8 = 0x12;
    pub const NO_VALUE: u8 = 0x13;
    pub const INTEGER: u8 = 0x21;
    pub const BOOLEAN: u8 = 0x22;
    pub const ENUM: u8 = 0x23;
    pub const OCTET_STRING: u8 = 0x30;
    pub const DATE_TIME: u8 = 0x31;
    pub const RESOLUTION: u8 = 0x32;
    pub const RANGE: u8 = 0x33;
    pub const BEGIN_COLLECTION: u8 = 0x34;
    pub const TEXT_WITH_LANGUAGE: u8 = 0x35;
    pub const NAME_WITH_LANGUAGE: u8 = 0x36;
    pub const END_COLLECTION: u8 = 0x37;
    pub const TEXT: u8 = 0x41;
    pub const NAME: u8 = 0x42;
    pub const KEYWORD: u8 = 0x44;
    pub const URI: u8 = 0x45;
    pub const URI_SCHEME: u8 = 0x46;
    pub const CHARSET: u8 = 0x47;
    pub const LANGUAGE: u8 = 0x48;
    pub const MIME_TYPE: u8 = 0x49;
    pub const MEMBER_NAME: u8 = 0x4A;
}

pub mod operation {
    pub const PRINT_JOB: u16 = 0x0002;
    pub const CANCEL_JOB: u16 = 0x0008;
    pub const GET_JOB_ATTRIBUTES: u16 = 0x0009;
    pub const GET_PRINTER_ATTRIBUTES: u16 = 0x000B;
    pub const CUPS_GET_DEFAULT: u16 = 0x4001;
    pub const CUPS_GET_PRINTERS: u16 = 0x4002;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Value {
    Integer(i32),
    Boolean(bool),
    Enum(i32),
    Text(u8, String),
    Resolution(i32, i32, u8),
    Range(i32, i32),
    Collection(Vec<Attribute>),
    Other(u8, Vec<u8>),
}

impl Value {
    #[must_use]
    pub fn text(&self) -> Option<&str> {
        match self {
            Self::Text(_, text) => Some(text),
            _ => None,
        }
    }

    #[must_use]
    pub const fn integer(&self) -> Option<i32> {
        match self {
            Self::Integer(value) | Self::Enum(value) => Some(*value),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Attribute {
    pub name: String,
    pub values: Vec<Value>,
}

impl Attribute {
    #[must_use]
    pub fn value(&self) -> Option<&Value> {
        self.values.first()
    }

    #[must_use]
    pub fn member<'a>(collection: &'a [Self], name: &str) -> Option<&'a Value> {
        collection
            .iter()
            .find(|attribute| attribute.name == name)
            .and_then(Self::value)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Group {
    pub tag: u8,
    pub attributes: Vec<Attribute>,
}

impl Group {
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&Attribute> {
        self.attributes
            .iter()
            .find(|attribute| attribute.name == name)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Message {
    pub version: (u8, u8),
    pub code: u16,
    pub request_id: u32,
    pub groups: Vec<Group>,
}

impl Message {
    #[must_use]
    pub fn request(operation: u16, request_id: u32) -> Self {
        Self {
            version: (2, 0),
            code: operation,
            request_id,
            groups: vec![Group {
                tag: tag::OPERATION,
                attributes: vec![
                    text("attributes-charset", tag::CHARSET, "utf-8"),
                    text("attributes-natural-language", tag::LANGUAGE, "en"),
                ],
            }],
        }
    }

    pub fn add(&mut self, group_tag: u8, attribute: Attribute) {
        if let Some(group) = self
            .groups
            .iter_mut()
            .rev()
            .find(|group| group.tag == group_tag)
        {
            group.attributes.push(attribute);
        } else {
            self.groups.push(Group {
                tag: group_tag,
                attributes: vec![attribute],
            });
        }
    }

    pub fn groups_of(&self, group_tag: u8) -> impl Iterator<Item = &Group> {
        self.groups
            .iter()
            .filter(move |group| group.tag == group_tag)
    }

    #[must_use]
    pub const fn succeeded(&self) -> bool {
        self.code <= 0x00FF
    }

    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut out = vec![self.version.0, self.version.1];
        out.extend_from_slice(&self.code.to_be_bytes());
        out.extend_from_slice(&self.request_id.to_be_bytes());
        for group in &self.groups {
            out.push(group.tag);
            for attribute in &group.attributes {
                encode_attribute(&mut out, attribute);
            }
        }
        out.push(tag::END);
        out
    }

    pub fn decode(bytes: &[u8]) -> Result<(Self, usize), IppError> {
        let mut reader = Reader { bytes, at: 0 };
        let version = (reader.byte()?, reader.byte()?);
        let code = reader.u16()?;
        let request_id = reader.u32()?;
        let mut groups: Vec<Group> = Vec::new();
        loop {
            let next = reader.byte()?;
            if next == tag::END {
                break;
            }
            if next < 0x10 {
                groups.push(Group {
                    tag: next,
                    attributes: Vec::new(),
                });
                continue;
            }
            let group = groups.last_mut().ok_or(IppError::Malformed)?;
            let name = reader.string()?;
            let value = reader.value(next)?;
            if name.is_empty() {
                let last = group.attributes.last_mut().ok_or(IppError::Malformed)?;
                last.values.push(value);
            } else {
                group.attributes.push(Attribute {
                    name,
                    values: vec![value],
                });
            }
        }
        Ok((
            Self {
                version,
                code,
                request_id,
                groups,
            },
            reader.at,
        ))
    }
}

#[must_use]
pub fn text(name: &str, value_tag: u8, value: &str) -> Attribute {
    Attribute {
        name: name.to_owned(),
        values: vec![Value::Text(value_tag, value.to_owned())],
    }
}

#[must_use]
pub fn integer(name: &str, value: i32) -> Attribute {
    Attribute {
        name: name.to_owned(),
        values: vec![Value::Integer(value)],
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IppError {
    Short,
    Malformed,
    TooDeep,
}

impl std::fmt::Display for IppError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Short => "the print server's answer ends early",
            Self::Malformed => "the print server's answer is not IPP",
            Self::TooDeep => "the print server's answer nests too deeply",
        })
    }
}

impl std::error::Error for IppError {}

fn put_string(out: &mut Vec<u8>, bytes: &[u8]) {
    let length = u16::try_from(bytes.len()).unwrap_or(u16::MAX);
    out.extend_from_slice(&length.to_be_bytes());
    out.extend_from_slice(&bytes[..usize::from(length)]);
}

fn encode_attribute(out: &mut Vec<u8>, attribute: &Attribute) {
    for (at, value) in attribute.values.iter().enumerate() {
        let name = if at == 0 {
            attribute.name.as_bytes()
        } else {
            b""
        };
        encode_value(out, name, value);
    }
}

fn encode_value(out: &mut Vec<u8>, name: &[u8], value: &Value) {
    match value {
        Value::Integer(number) | Value::Enum(number) => {
            out.push(if matches!(value, Value::Enum(_)) {
                tag::ENUM
            } else {
                tag::INTEGER
            });
            put_string(out, name);
            put_string(out, &number.to_be_bytes());
        }
        Value::Boolean(flag) => {
            out.push(tag::BOOLEAN);
            put_string(out, name);
            put_string(out, &[u8::from(*flag)]);
        }
        Value::Text(value_tag, text) => {
            out.push(*value_tag);
            put_string(out, name);
            put_string(out, text.as_bytes());
        }
        Value::Resolution(across, down, units) => {
            out.push(tag::RESOLUTION);
            put_string(out, name);
            let mut bytes = across.to_be_bytes().to_vec();
            bytes.extend_from_slice(&down.to_be_bytes());
            bytes.push(*units);
            put_string(out, &bytes);
        }
        Value::Range(low, high) => {
            out.push(tag::RANGE);
            put_string(out, name);
            let mut bytes = low.to_be_bytes().to_vec();
            bytes.extend_from_slice(&high.to_be_bytes());
            put_string(out, &bytes);
        }
        Value::Collection(members) => {
            out.push(tag::BEGIN_COLLECTION);
            put_string(out, name);
            put_string(out, b"");
            for member in members {
                for (at, member_value) in member.values.iter().enumerate() {
                    if at == 0 {
                        out.push(tag::MEMBER_NAME);
                        put_string(out, b"");
                        put_string(out, member.name.as_bytes());
                    }
                    encode_value(out, b"", member_value);
                }
            }
            out.push(tag::END_COLLECTION);
            put_string(out, b"");
            put_string(out, b"");
        }
        Value::Other(value_tag, bytes) => {
            out.push(*value_tag);
            put_string(out, name);
            put_string(out, bytes);
        }
    }
}

const MOST_DEPTH: usize = 8;

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl Reader<'_> {
    fn take(&mut self, count: usize) -> Result<&[u8], IppError> {
        let end = self.at.checked_add(count).ok_or(IppError::Short)?;
        let taken = self.bytes.get(self.at..end).ok_or(IppError::Short)?;
        self.at = end;
        Ok(taken)
    }

    fn byte(&mut self) -> Result<u8, IppError> {
        Ok(self.take(1)?[0])
    }

    fn u16(&mut self) -> Result<u16, IppError> {
        let bytes = self.take(2)?;
        Ok(u16::from_be_bytes([bytes[0], bytes[1]]))
    }

    fn u32(&mut self) -> Result<u32, IppError> {
        let bytes = self.take(4)?;
        Ok(u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    fn string(&mut self) -> Result<String, IppError> {
        let length = usize::from(self.u16()?);
        Ok(String::from_utf8_lossy(self.take(length)?).into_owned())
    }

    fn raw(&mut self) -> Result<Vec<u8>, IppError> {
        let length = usize::from(self.u16()?);
        Ok(self.take(length)?.to_vec())
    }

    fn value(&mut self, value_tag: u8) -> Result<Value, IppError> {
        self.value_at(value_tag, 0)
    }

    fn value_at(&mut self, value_tag: u8, depth: usize) -> Result<Value, IppError> {
        if value_tag == tag::BEGIN_COLLECTION {
            let _ = self.raw()?;
            return self.collection(depth + 1);
        }
        let bytes = self.raw()?;
        let four = |bytes: &[u8]| -> Result<i32, IppError> {
            let array: [u8; 4] = bytes.try_into().map_err(|_| IppError::Malformed)?;
            Ok(i32::from_be_bytes(array))
        };
        Ok(match value_tag {
            tag::INTEGER => Value::Integer(four(&bytes)?),
            tag::ENUM => Value::Enum(four(&bytes)?),
            tag::BOOLEAN => Value::Boolean(bytes.first().is_some_and(|flag| *flag != 0)),
            tag::RESOLUTION if bytes.len() == 9 => {
                Value::Resolution(four(&bytes[0..4])?, four(&bytes[4..8])?, bytes[8])
            }
            tag::RANGE if bytes.len() == 8 => {
                Value::Range(four(&bytes[0..4])?, four(&bytes[4..8])?)
            }
            tag::TEXT
            | tag::NAME
            | tag::KEYWORD
            | tag::URI
            | tag::URI_SCHEME
            | tag::CHARSET
            | tag::LANGUAGE
            | tag::MIME_TYPE => {
                Value::Text(value_tag, String::from_utf8_lossy(&bytes).into_owned())
            }
            other => Value::Other(other, bytes),
        })
    }

    fn collection(&mut self, depth: usize) -> Result<Value, IppError> {
        if depth > MOST_DEPTH {
            return Err(IppError::TooDeep);
        }
        let mut members: Vec<Attribute> = Vec::new();
        loop {
            let value_tag = self.byte()?;
            let _name = self.raw()?;
            match value_tag {
                tag::END_COLLECTION => {
                    let _ = self.raw()?;
                    return Ok(Value::Collection(members));
                }
                tag::MEMBER_NAME => {
                    let member = String::from_utf8_lossy(&self.raw()?).into_owned();
                    let member_tag = self.byte()?;
                    let _ = self.raw()?;
                    let value = self.member_value(member_tag, depth)?;
                    members.push(Attribute {
                        name: member,
                        values: vec![value],
                    });
                }
                other => {
                    let value = self.member_value(other, depth)?;
                    members
                        .last_mut()
                        .ok_or(IppError::Malformed)?
                        .values
                        .push(value);
                }
            }
        }
    }

    fn member_value(&mut self, value_tag: u8, depth: usize) -> Result<Value, IppError> {
        if value_tag == tag::BEGIN_COLLECTION {
            let _ = self.raw()?;
            self.collection(depth + 1)
        } else {
            self.value_at(value_tag, depth)
        }
    }
}
