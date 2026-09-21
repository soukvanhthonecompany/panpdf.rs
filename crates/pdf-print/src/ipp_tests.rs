use crate::ipp::{Attribute, Message, Value, integer, operation, tag, text};

#[test]
fn the_rfc_print_job_request_is_encoded_byte_for_byte() {
    let mut request = Message::request(operation::PRINT_JOB, 1);
    request.add(
        tag::OPERATION,
        text(
            "printer-uri",
            tag::URI,
            "ipp://printer.example.com/ipp/print/pinetree",
        ),
    );
    request.add(tag::OPERATION, text("job-name", tag::NAME, "foobar"));
    request.add(
        tag::OPERATION,
        Attribute {
            name: "ipp-attribute-fidelity".to_owned(),
            values: vec![Value::Boolean(true)],
        },
    );
    request.add(tag::JOB, integer("copies", 20));
    request.add(tag::JOB, text("sides", tag::KEYWORD, "two-sided-long-edge"));
    let mut expected: Vec<u8> = vec![0x02, 0x00, 0x00, 0x02, 0, 0, 0, 1, 0x01];
    let mut put = |value_tag: u8, name: &str, value: &[u8]| {
        expected.push(value_tag);
        expected.extend_from_slice(&u16::try_from(name.len()).expect("short").to_be_bytes());
        expected.extend_from_slice(name.as_bytes());
        expected.extend_from_slice(&u16::try_from(value.len()).expect("short").to_be_bytes());
        expected.extend_from_slice(value);
    };
    put(0x47, "attributes-charset", b"utf-8");
    put(0x48, "attributes-natural-language", b"en");
    put(
        0x45,
        "printer-uri",
        b"ipp://printer.example.com/ipp/print/pinetree",
    );
    put(0x42, "job-name", b"foobar");
    put(0x22, "ipp-attribute-fidelity", &[1]);
    expected.push(0x02);
    let mut put = |value_tag: u8, name: &str, value: &[u8]| {
        expected.push(value_tag);
        expected.extend_from_slice(&u16::try_from(name.len()).expect("short").to_be_bytes());
        expected.extend_from_slice(name.as_bytes());
        expected.extend_from_slice(&u16::try_from(value.len()).expect("short").to_be_bytes());
        expected.extend_from_slice(value);
    };
    put(0x21, "copies", &20_i32.to_be_bytes());
    put(0x44, "sides", b"two-sided-long-edge");
    expected.push(0x03);
    assert_eq!(request.encode(), expected);
}

#[test]
fn a_message_reads_back_as_itself() {
    let mut message = Message::request(operation::GET_PRINTER_ATTRIBUTES, 7);
    message.code = 0x0000;
    let size = Value::Collection(vec![
        integer("x-dimension", 20997),
        integer("y-dimension", 29697),
    ]);
    let media = Value::Collection(vec![
        Attribute {
            name: "media-size".to_owned(),
            values: vec![size],
        },
        integer("media-top-margin", 296),
        Attribute {
            name: "media-source".to_owned(),
            values: vec![
                Value::Text(tag::KEYWORD, "main".to_owned()),
                Value::Text(tag::KEYWORD, "rear".to_owned()),
            ],
        },
    ]);
    message.add(
        tag::PRINTER,
        Attribute {
            name: "media-col-database".to_owned(),
            values: vec![media.clone(), media],
        },
    );
    message.add(
        tag::PRINTER,
        Attribute {
            name: "media-supported".to_owned(),
            values: vec![
                Value::Text(tag::KEYWORD, "iso_a4_210x297mm".to_owned()),
                Value::Text(tag::KEYWORD, "na_letter_8.5x11in".to_owned()),
            ],
        },
    );
    message.add(
        tag::PRINTER,
        Attribute {
            name: "copies-supported".to_owned(),
            values: vec![Value::Range(1, 9999)],
        },
    );
    message.add(
        tag::PRINTER,
        Attribute {
            name: "printer-resolution-default".to_owned(),
            values: vec![Value::Resolution(300, 300, 3)],
        },
    );
    let mut bytes = message.encode();
    bytes.extend_from_slice(b"%PDF");
    let (read, end) = Message::decode(&bytes).expect("reads");
    assert_eq!(read, message);
    assert_eq!(
        &bytes[end..],
        b"%PDF",
        "the document begins after the end tag"
    );
}

#[test]
fn what_is_not_ipp_is_refused() {
    let whole = Message::request(operation::CUPS_GET_PRINTERS, 1).encode();
    for cut in 0..whole.len() {
        assert!(Message::decode(&whole[..cut]).is_err(), "cut at {cut}");
    }
    let orphan = [
        2, 0, 0, 0, 0, 0, 0, 1, 0x21, 0, 1, b'x', 0, 4, 0, 0, 0, 1, 3,
    ];
    assert!(Message::decode(&orphan).is_err());
}

#[test]
fn a_status_below_0x100_is_success() {
    let mut message = Message::request(0, 1);
    message.code = 0x0001;
    assert!(message.succeeded());
    message.code = 0x0406;
    assert!(!message.succeeded());
}
