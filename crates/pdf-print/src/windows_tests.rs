use std::sync::atomic::AtomicBool;

use crate::layout::Sheet;
use crate::sheet::SheetImage;
use crate::windows::{WindowsError, capabilities_from, printers_from, sheet_bytes, stream_sheets};

#[test]
fn sheet_bytes_is_a_known_answer() {
    let image = SheetImage {
        width: 3,
        height: 2,
        rgb: vec![
            10, 20, 30, 40, 50, 60, 70, 80, 90, 100, 110, 120, 130, 140, 150, 160, 170, 180,
        ],
    };
    let bytes = sheet_bytes([216.0, 144.0], &image);
    let mut expected = Vec::new();
    expected.extend_from_slice(&3_u32.to_le_bytes());
    expected.extend_from_slice(&2_u32.to_le_bytes());
    expected.extend_from_slice(&216.0_f64.to_le_bytes());
    expected.extend_from_slice(&144.0_f64.to_le_bytes());
    expected.extend_from_slice(&[30, 20, 10, 60, 50, 40, 90, 80, 70, 0, 0, 0]);
    expected.extend_from_slice(&[120, 110, 100, 150, 140, 130, 180, 170, 160, 0, 0, 0]);
    assert_eq!(bytes, expected);
    assert_eq!(bytes.len(), 24 + 12 * image.height as usize);
}

fn read_sheet(bytes: &[u8], at: usize) -> (u32, u32, [f64; 2], usize) {
    let width = u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap());
    let height = u32::from_le_bytes(bytes[at + 4..at + 8].try_into().unwrap());
    let across = f64::from_le_bytes(bytes[at + 8..at + 16].try_into().unwrap());
    let down = f64::from_le_bytes(bytes[at + 16..at + 24].try_into().unwrap());
    let row = (width as usize * 3 + 3) & !3;
    (
        width,
        height,
        [across, down],
        at + 24 + row * height as usize,
    )
}

#[test]
fn stream_sheets_writes_the_count_then_each_sheet_in_order() {
    let sheets = vec![
        Sheet {
            size: [72.0, 144.0],
            placements: Vec::new(),
        },
        Sheet {
            size: [36.0, 72.0],
            placements: Vec::new(),
        },
    ];
    let mut out = Vec::new();
    let mut drawn = Vec::new();
    let stop = AtomicBool::new(false);
    stream_sheets(
        &mut out,
        &sheets,
        (72.0, false),
        |_, _, _| panic!("no page is placed on either sheet"),
        (|at| drawn.push(at), &stop),
    )
    .expect("streams");
    assert_eq!(u32::from_le_bytes(out[0..4].try_into().unwrap()), 2);
    let (width, height, size, after) = read_sheet(&out, 4);
    assert_eq!((width, height, size), (72, 144, [72.0, 144.0]));
    let (width, height, size, after) = read_sheet(&out, after);
    assert_eq!((width, height, size), (36, 72, [36.0, 72.0]));
    assert_eq!(after, out.len());
    assert_eq!(drawn, vec![1, 2]);
}

#[test]
fn a_stop_set_before_the_first_sheet_draws_nothing() {
    let sheets = vec![Sheet {
        size: [100.0, 200.0],
        placements: Vec::new(),
    }];
    let stop = AtomicBool::new(true);
    let mut out = Vec::new();
    let mut drawn = 0;
    let result = stream_sheets(
        &mut out,
        &sheets,
        (72.0, false),
        |_, _, _| panic!("a stopped job draws nothing"),
        (|_| drawn += 1, &stop),
    );
    assert!(matches!(result, Err(crate::job::JobError::Stopped)));
    assert_eq!(drawn, 0);
}

#[test]
fn printers_from_reads_base64_names_and_the_default() {
    let answer = "printer\tRnJvbnQgT2ZmaWNl\n\
        printer\t4LmA4LiE4Lij4Li34LmI4Lit4LiH4Lie4Li04Lih4Lie4LmM\n\
        default\t4LmA4LiE4Lij4Li34LmI4Lit4LiH4Lie4Li04Lih4Lie4LmM\n";
    let (printers, default) = printers_from(answer).expect("parses");
    let names: Vec<&str> = printers
        .iter()
        .map(|printer| printer.name.as_str())
        .collect();
    assert_eq!(names, vec!["Front Office", "เครื่องพิมพ์"]);
    assert_eq!(default.as_deref(), Some("เครื่องพิมพ์"));
}

#[test]
fn an_error_line_becomes_refused_with_its_decoded_message() {
    let answer =
        "error\tVGhpcyBwcmludGVyIGRvZXMgbm90IHByaW50IG9uIGJvdGggc2lkZXMgb2YgdGhlIHBhcGVyLg==\n";
    let result = printers_from(answer);
    assert!(matches!(
        result,
        Err(WindowsError::Refused(message))
            if message == "This printer does not print on both sides of the paper."
    ));
}

#[test]
fn an_unrecognised_capability_line_becomes_garbled() {
    let answer = "colour\t1\nresolution\t600\nmystery\t1\t2\n";
    assert!(matches!(
        capabilities_from(answer),
        Err(WindowsError::Garbled(_))
    ));
}

#[test]
fn capabilities_from_reads_the_ask_answer() {
    let answer = "colour\t1\n\
        two-sided\t1\n\
        resolution\t600\n\
        default-paper\t827\t1169\n\
        paper\t827\t1169\n\
        margin\t827\t1169\t50\t30\t40\t100\n";
    let found = capabilities_from(answer).expect("parses");
    assert!(found.colour);
    assert!(found.two_sided);
    assert_eq!(found.resolution, Some(600));
    assert_eq!(found.media, vec!["iso_a4_210x297mm".to_owned()]);
    assert_eq!(found.media_default, Some("iso_a4_210x297mm".to_owned()));
    assert_eq!(
        found.margins,
        vec![([21005, 29692], [2540, 1270, 1016, 762])]
    );
    assert_eq!(found.margin_for([21000, 29700]), Some(2540));
}
