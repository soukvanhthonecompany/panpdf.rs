use super::{Element, Reader, retagged, tag};

#[test]
fn a_length_written_in_several_octets_reads_as_one_number() {
    let mut body = vec![tag::OCTET_STRING, 0x82, 0x01, 0x00];
    body.extend(std::iter::repeat_n(0x41, 256));
    let element = Reader::new(&body).next().expect("it reads");
    assert_eq!(element.content.len(), 256);
    assert_eq!(
        element.whole.len(),
        260,
        "the header belongs to the element"
    );
}

#[test]
fn an_element_that_states_no_length_ends_where_its_zeros_are() {
    let body = [
        tag::SEQUENCE,
        0x80,
        tag::INTEGER,
        0x01,
        0x07,
        tag::OCTET_STRING,
        0x02,
        b'h',
        b'i',
        0x00,
        0x00,
    ];
    let element = Reader::new(&body).next().expect("it reads");
    assert!(element.unstated_length, "it is remembered as what it is");
    assert_eq!(element.whole, &body[..], "the two zeros belong to it");
    assert_eq!(
        element.content.len(),
        7,
        "and they are not part of what is inside it"
    );
    let mut inside = element.reader();
    assert_eq!(
        inside.expect(tag::INTEGER).expect("first").unsigned(),
        Ok(&[7][..])
    );
    assert_eq!(
        inside.expect(tag::OCTET_STRING).expect("second").content,
        b"hi"
    );
    assert!(inside.is_empty(), "and nothing is left over");
}

#[test]
fn an_element_that_never_ends_is_refused() {
    let body = [tag::SEQUENCE, 0x80, tag::INTEGER, 0x01, 0x07];
    assert!(Reader::new(&body).next().is_err());
}

#[test]
fn a_length_past_the_end_is_refused() {
    let body = [tag::OCTET_STRING, 0x10, 0x00, 0x00];
    assert!(Reader::new(&body).next().is_err());
}

#[test]
fn what_follows_a_structure_is_left_alone() {
    let mut body = vec![tag::SEQUENCE, 0x03, tag::INTEGER, 0x01, 0x09];
    body.extend_from_slice(&[0; 40]);
    let element = Reader::new(&body).next().expect("it reads");
    assert_eq!(element.whole.len(), 5, "the padding is not part of it");
}

#[test]
fn an_unsigned_number_loses_the_zero_that_keeps_it_positive() {
    let big = [tag::INTEGER, 0x02, 0x00, 0xff];
    assert_eq!(
        Reader::new(&big).next().expect("read").unsigned(),
        Ok(&[0xff][..])
    );
    let zero = [tag::INTEGER, 0x01, 0x00];
    assert_eq!(
        Reader::new(&zero).next().expect("read").unsigned(),
        Ok(&[0][..]),
        "zero is a number and keeps an octet to be one"
    );
    let nothing = [tag::INTEGER, 0x00];
    assert!(
        Reader::new(&nothing)
            .next()
            .expect("read")
            .unsigned()
            .is_err(),
        "an integer with no octets says nothing"
    );
}

#[test]
fn a_bit_string_that_does_not_end_on_an_octet_is_refused() {
    let whole = [tag::BIT_STRING, 0x03, 0x00, 0xab, 0xcd];
    assert_eq!(
        Reader::new(&whole).next().expect("read").bits(),
        Ok(&[0xab, 0xcd][..])
    );
    let part = [tag::BIT_STRING, 0x02, 0x04, 0xa0];
    assert!(Reader::new(&part).next().expect("read").bits().is_err());
}

#[test]
fn an_optional_element_that_is_absent_takes_nothing() {
    let body = [
        tag::SEQUENCE,
        0x06,
        tag::INTEGER,
        0x01,
        0x01,
        tag::INTEGER,
        0x01,
        0x02,
    ];
    let element = Reader::new(&body).next().expect("read");
    let mut inside = element.reader();
    assert!(inside.optional(tag::CONTEXT_0).is_none());
    assert_eq!(inside.peek(), Some(tag::INTEGER));
    assert_eq!(
        inside.expect(tag::INTEGER).expect("first").unsigned(),
        Ok(&[1][..])
    );
    assert_eq!(
        inside.expect(tag::INTEGER).expect("second").unsigned(),
        Ok(&[2][..])
    );
}

#[test]
fn contents_written_out_again_under_a_different_tag() {
    let short = retagged(tag::SET, b"ab");
    assert_eq!(short, vec![tag::SET, 0x02, b'a', b'b']);
    let long = retagged(tag::SET, &[0x41; 300]);
    assert_eq!(&long[..4], &[tag::SET, 0x82, 0x01, 0x2c]);
    assert_eq!(long.len(), 304);
    let read = Reader::new(&long).next().expect("what it writes, it reads");
    assert_eq!(read.content.len(), 300);
}

#[test]
fn nesting_without_end_is_refused() {
    let mut body = Vec::new();
    for _ in 0..2_000 {
        body.push(tag::SEQUENCE);
        body.push(0x80);
    }
    body.extend(std::iter::repeat_n(0x00, 4_000));
    assert!(Reader::new(&body).next().is_err());
}

#[test]
fn an_identifier_this_does_not_read_is_refused() {
    let body = [0x1f, 0x81, 0x00, 0x00];
    assert!(Reader::new(&body).next().is_err());
    let _ = Element {
        tag: tag::NULL,
        content: &[],
        whole: &[],
        unstated_length: false,
    };
}
