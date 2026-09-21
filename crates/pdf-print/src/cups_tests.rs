use std::sync::atomic::AtomicBool;

use crate::cups::{Capabilities, JobSettings, Sides, http_body};

#[test]
fn a_chunked_answer_is_put_back_together() {
    let chunked = b"HTTP/1.1 200 OK\r\nContent-Type: application/ipp\r\n\
        Transfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n3\r\n ipp\r\n0\r\n\r\n";
    assert_eq!(http_body(chunked).expect("reads"), b"hello ip".to_vec());
    let plain = b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\n\r\nipp!";
    assert_eq!(http_body(plain).expect("reads"), b"ipp!".to_vec());
    let refused = b"HTTP/1.1 426 Upgrade Required\r\n\r\n";
    assert!(http_body(refused).is_err());
    assert!(http_body(b"not http").is_err());
    let cut = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n9\r\nshort\r\n";
    assert!(http_body(cut).is_err());
}

#[test]
fn the_printers_margin_is_the_widest_edge_of_that_paper() {
    let found = Capabilities {
        margins: vec![
            ([20997, 29697], [296, 296, 296, 400]),
            ([20997, 29697], [0, 0, 0, 0]),
            ([10499, 14803], [500, 500, 500, 500]),
        ],
        ..Capabilities::default()
    };
    assert_eq!(found.margin_for([21000, 29700]), Some(400));
    assert_eq!(found.margin_for([10500, 14800]), Some(500));
    assert_eq!(
        found.margin_for([29700, 42000]),
        None,
        "a paper it has not got"
    );
}

#[test]
fn a_stopped_job_draws_nothing_and_sends_nothing() {
    let sheets =
        crate::lay_out(&[(0, [100.0, 200.0])], &crate::Settings::default()).expect("lays out");
    let stopped = AtomicBool::new(true);
    let mut drawn = 0;
    let answer = crate::job::print_sheets(
        &sheets,
        (72.0, false),
        |_, _, _| panic!("a stopped job draws nothing"),
        (
            "no-such-printer",
            &JobSettings {
                title: "stopped".to_owned(),
                copies: 1,
                media: "iso_a4_210x297mm".to_owned(),
                colour: true,
                hold: true,
                sides: Sides::One,
            },
        ),
        (
            |_| {
                drawn += 1;
            },
            &stopped,
        ),
    );
    assert!(matches!(answer, Err(crate::job::JobError::Stopped)));
    assert_eq!(drawn, 0);
}
