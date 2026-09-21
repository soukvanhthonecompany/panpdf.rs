use std::path::{Path, PathBuf};

use crate::desk::Desk;
use crate::json::Json;

fn document(objects: &[String]) -> Vec<u8> {
    let mut bytes = b"%PDF-1.7\n%\xe2\xe3\xcf\xd3\n".to_vec();
    let mut offsets = Vec::new();
    for (at, object) in objects.iter().enumerate() {
        offsets.push(bytes.len());
        bytes.extend_from_slice(format!("{} 0 obj\n{object}\nendobj\n", at + 1).as_bytes());
    }
    let table = bytes.len();
    bytes.extend_from_slice(
        format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes(),
    );
    for offset in &offsets {
        bytes.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    bytes.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{table}\n%%EOF\n",
            objects.len() + 1
        )
        .as_bytes(),
    );
    bytes
}

fn a_form() -> Vec<u8> {
    document(&[
        "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [4 0 R 5 0 R 6 0 R 9 0 R 10 0 R] \
         /DA (/Helv 0 Tf 0 g) >> >>"
            .to_owned(),
        "<< /Type /Pages /MediaBox [0 0 300 300] /Kids [3 0 R] /Count 1 >>".to_owned(),
        "<< /Type /Page /Parent 2 0 R /Contents 11 0 R /Resources << /ProcSet [/PDF] >> \
         /Annots [4 0 R 5 0 R 7 0 R 8 0 R 9 0 R 10 0 R] >>"
            .to_owned(),
        "<< /Type /Annot /Subtype /Widget /FT /Tx /T (Name) /Rect [20 200 120 220] \
         /DA (/Helv 10 Tf 0 g) >>"
            .to_owned(),
        "<< /Type /Annot /Subtype /Widget /FT /Btn /T (Agree) /V /Off /AS /Off \
         /Rect [20 160 36 176] /AP << /N << /Yes 12 0 R /Off 12 0 R >> >> >>"
            .to_owned(),
        "<< /FT /Btn /Ff 32768 /T (Size) /V /Off /Kids [7 0 R 8 0 R] >>".to_owned(),
        "<< /Type /Annot /Subtype /Widget /Parent 6 0 R /Rect [20 120 36 136] /AS /Off \
         /AP << /N << /Large 12 0 R /Off 12 0 R >> >> >>"
            .to_owned(),
        "<< /Type /Annot /Subtype /Widget /Parent 6 0 R /Rect [50 120 66 136] /AS /Off \
         /AP << /N << /Small 12 0 R /Off 12 0 R >> >> >>"
            .to_owned(),
        "<< /Type /Annot /Subtype /Widget /FT /Tx /Ff 1 /T (Reference) /V (RF-1) \
         /Rect [20 80 120 100] /DA (/Helv 10 Tf 0 g) >>"
            .to_owned(),
        "<< /Type /Annot /Subtype /Widget /FT /Ch /T (Paper) /Opt [(A4) (A3) (Letter)] \
         /Rect [20 40 120 60] /DA (/Helv 10 Tf 0 g) >>"
            .to_owned(),
        "<< /Length 0 >>\nstream\n\nendstream".to_owned(),
        "<< /Type /XObject /Subtype /Form /BBox [0 0 16 16] /Length 0 >>\nstream\n\nendstream"
            .to_owned(),
    ])
}

fn folder(name: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!("panpdf-about-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).expect("a folder");
    path
}

fn opened(folder: &Path) -> (Desk, String) {
    let path = folder.join("form.pdf");
    std::fs::write(&path, a_form()).expect("written");
    let mut desk = Desk::with_fonts(Some(crate::desk::tests::fonts()));
    let handle = desk.open(&path, "", false).expect("opens").handle;
    (desk, handle)
}

fn value_of(desk: &mut Desk, handle: &str, name: &str) -> String {
    let (source, credential) = desk.source(handle).expect("the document");
    let fields = pdf_edit::form::fields_of_document(&source, &credential).expect("the form");
    let field = fields
        .iter()
        .find(|(_, field)| field.name == name)
        .unwrap_or_else(|| panic!("the form has no {name}"));
    match &field.1.value {
        pdf_edit::form::FieldValue::Text(text) => text.clone(),
        pdf_edit::form::FieldValue::State(state) => format!("/{state}"),
        pdf_edit::form::FieldValue::Empty => String::new(),
    }
}

#[test]
fn a_text_field_says_what_was_typed_into_it() {
    let folder = folder("text");
    let (mut desk, handle) = opened(&folder);
    let answer = super::fill_field(
        &mut desk,
        &handle,
        "Name",
        &Json::text("Somchai".to_owned()),
    )
    .expect("the field fills");
    assert!(answer.text.contains("Name"), "{}", answer.text);
    assert_eq!(value_of(&mut desk, &handle, "Name"), "Somchai");
}

#[test]
fn a_checkbox_is_ticked_and_cleared_by_its_own_names() {
    let folder = folder("tick");
    let (mut desk, handle) = opened(&folder);
    super::fill_field(&mut desk, &handle, "Agree", &Json::Bool(true)).expect("ticked");
    assert_eq!(value_of(&mut desk, &handle, "Agree"), "/Yes");
    super::fill_field(&mut desk, &handle, "Agree", &Json::Bool(false)).expect("cleared");
    assert_eq!(value_of(&mut desk, &handle, "Agree"), "/Off");
}

#[test]
fn a_radio_group_sets_the_button_that_holds_the_state() {
    let folder = folder("radio");
    let (mut desk, handle) = opened(&folder);
    super::fill_field(&mut desk, &handle, "Size", &Json::text("Small".to_owned()))
        .expect("the small button is chosen");
    assert_eq!(value_of(&mut desk, &handle, "Size"), "/Small");
    super::fill_field(&mut desk, &handle, "Size", &Json::text("Large".to_owned()))
        .expect("the large button is chosen");
    assert_eq!(value_of(&mut desk, &handle, "Size"), "/Large");
    let refused = super::fill_field(&mut desk, &handle, "Size", &Json::text("Huge".to_owned()))
        .err()
        .expect("no button holds Huge");
    assert!(
        refused.contains("Large") && refused.contains("Small"),
        "{refused}"
    );
}

#[test]
fn a_read_only_field_is_refused_and_keeps_its_value() {
    let folder = folder("read-only");
    let (mut desk, handle) = opened(&folder);
    let refused = super::fill_field(
        &mut desk,
        &handle,
        "Reference",
        &Json::text("RF-2".to_owned()),
    )
    .err()
    .expect("read-only");
    assert!(refused.contains("read-only"), "{refused}");
    assert_eq!(value_of(&mut desk, &handle, "Reference"), "RF-1");
}

#[test]
fn a_choice_takes_one_of_its_own_options() {
    let folder = folder("choice");
    let (mut desk, handle) = opened(&folder);
    super::fill_field(&mut desk, &handle, "Paper", &Json::text("A3".to_owned()))
        .expect("A3 is offered");
    assert_eq!(value_of(&mut desk, &handle, "Paper"), "A3");
    let refused = super::fill_field(&mut desk, &handle, "Paper", &Json::text("A2".to_owned()))
        .err()
        .expect("A2 is not offered");
    assert!(
        refused.contains("A4") && refused.contains("Letter"),
        "{refused}"
    );
}

#[test]
fn document_info_lists_the_pages_and_the_form() {
    let folder = folder("info");
    let (mut desk, handle) = opened(&folder);
    super::fill_field(
        &mut desk,
        &handle,
        "Name",
        &Json::text("Somchai".to_owned()),
    )
    .expect("filled");
    let answer = super::document_info(&mut desk, &handle).expect("the facts read");
    assert!(answer.text.contains("Name"), "{}", answer.text);
    let fields = answer
        .data
        .get("fields")
        .and_then(Json::as_list)
        .expect("the form's fields");
    let named: Vec<&str> = fields
        .iter()
        .filter_map(|field| field.get("name").and_then(Json::as_str))
        .collect();
    for wanted in ["Name", "Agree", "Size", "Reference", "Paper"] {
        assert!(named.contains(&wanted), "{wanted} in {named:?}");
    }
    let name = fields
        .iter()
        .find(|field| field.get("name").and_then(Json::as_str) == Some("Name"))
        .expect("the text field");
    assert_eq!(name.get("value").and_then(Json::as_str), Some("Somchai"));
}
