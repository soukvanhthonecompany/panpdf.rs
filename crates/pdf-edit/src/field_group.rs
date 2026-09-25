use pdf_bytes::ByteStore;
use pdf_syntax::Reference;

use crate::fill_field::{Entry, set_entries};
use crate::form::{FieldKind, FormField};
use crate::new_field::{NewField, NewFieldKind};
use crate::plan::{Capability, Effect, Plan, PlannedWrite};
use crate::spike_move_text::{PlannerPage, SpikeError};

pub const MOST_FIELDS: usize = 1_000;

fn refused(reason: &'static str) -> SpikeError {
    SpikeError::RetypeUnsupported(reason)
}

pub(crate) fn in_sequence<T>(
    (source, credential): (&ByteStore, &[u8]),
    items: &[T],
    mut step: impl FnMut(&ByteStore, &T) -> Result<Vec<PlannedWrite>, SpikeError>,
) -> Result<Vec<PlannedWrite>, SpikeError> {
    if items.is_empty() {
        return Err(refused("nothing was named"));
    }
    if items.len() > MOST_FIELDS {
        return Err(refused("this is more than one change takes"));
    }
    let security =
        crate::previous::readable_index(source, credential).and_then(|(_, security)| security);
    let mut document = source.clone();
    let mut writes: Vec<PlannedWrite> = Vec::new();
    for item in items {
        let mut made = step(&document, item)?;
        if let Some(security) = &security {
            for write in &mut made {
                plain_again(security, (source, &document), write)?;
            }
        }
        document = crate::block_rewrite::commit_writes(
            &document,
            &made,
            (credential, crate::Restrictions::SetAside),
        )?;
        for write in made {
            match writes
                .iter_mut()
                .find(|held| held.reference == write.reference)
            {
                Some(held) => *held = write,
                None => writes.push(write),
            }
        }
    }
    Ok(writes)
}

fn plain_again(
    security: &pdf_security::AuthenticatedSecurity,
    (source, document): (&ByteStore, &ByteStore),
    write: &mut PlannedWrite,
) -> Result<(), SpikeError> {
    let crate::plan::PlannedBody::Direct { body } = &mut write.body else {
        return Ok(());
    };
    if crate::previous::defined(source, write.reference)?
        || !crate::previous::defined(document, write.reference)?
    {
        return Ok(());
    }
    *body = crate::incremental::with_strings_decrypted(security, write.reference, body)
        .map_err(SpikeError::Write)?;
    Ok(())
}

fn plan_of(
    page: &PlannerPage<'_>,
    page_index: usize,
    writes: Vec<PlannedWrite>,
    region: Option<[f64; 4]>,
) -> Plan {
    let target = page
        .program
        .streams
        .first()
        .map_or(page.program.page, |stream| stream.reference);
    Plan::new(
        Capability::Exact,
        writes,
        Effect {
            page_index,
            moved: Vec::new(),
            target_stream: target,
            declared_region: region,
        },
    )
}

fn union(boxes: impl IntoIterator<Item = [f64; 4]>) -> Option<[f64; 4]> {
    boxes.into_iter().reduce(|one, other| {
        [
            one[0].min(other[0]),
            one[1].min(other[1]),
            one[2].max(other[2]),
            one[3].max(other[3]),
        ]
    })
}

pub(crate) fn plan_set_field_boxes(
    source: &ByteStore,
    page: PlannerPage<'_>,
    page_index: usize,
    boxes: &[(Reference, [f64; 4])],
) -> Result<Plan, SpikeError> {
    let credential = page.credential;
    for (at, (widget, _)) in boxes.iter().enumerate() {
        if boxes[..at].iter().any(|(earlier, _)| earlier == widget) {
            return Err(refused("a field was named twice"));
        }
    }
    let before = crate::form::fields_of_page(source, page.program.page, credential)?;
    let writes = in_sequence((source, credential), boxes, |document, (widget, rect)| {
        let plan = crate::field_settings::plan_set_field_box(
            document,
            page,
            page_index,
            (*widget, *rect),
        )?;
        Ok(plan.writes().to_vec())
    })?;
    let region = union(
        boxes.iter().map(|(_, rect)| *rect).chain(
            before
                .iter()
                .filter(|field| boxes.iter().any(|(widget, _)| *widget == field.widget))
                .map(|field| field.rect),
        ),
    );
    Ok(plan_of(&page, page_index, writes, region))
}

pub(crate) fn plan_remove_fields(
    source: &ByteStore,
    page: PlannerPage<'_>,
    page_index: usize,
    widgets: &[Reference],
) -> Result<Plan, SpikeError> {
    let credential = page.credential;
    let before = crate::form::fields_of_page(source, page.program.page, credential)?;
    let region = union(
        before
            .iter()
            .filter(|field| widgets.contains(&field.widget))
            .map(|field| field.rect),
    );
    let writes = in_sequence((source, credential), widgets, |document, widget| {
        let plan = crate::new_field::plan_remove_field(document, page, page_index, *widget)?;
        Ok(plan.writes().to_vec())
    })?;
    Ok(plan_of(&page, page_index, writes, region))
}

fn copied_kind(field: &FormField) -> Result<NewFieldKind, SpikeError> {
    Ok(match field.kind {
        FieldKind::Text if field.multiline => NewFieldKind::Paragraph,
        FieldKind::Text => NewFieldKind::Text,
        FieldKind::Checkbox => NewFieldKind::Checkbox,
        FieldKind::Radio => NewFieldKind::Radio,
        FieldKind::Combo => NewFieldKind::Dropdown,
        FieldKind::List | FieldKind::Push | FieldKind::Signature => {
            return Err(refused("this kind of field cannot be copied yet"));
        }
    })
}

fn copy_name(field: &FormField, taken: &[String]) -> String {
    if field.kind == FieldKind::Radio {
        return field.name.clone();
    }
    let leaf = field.name.rsplit('.').next().unwrap_or(&field.name);
    let stem = leaf.trim_end_matches(|letter: char| letter.is_ascii_digit());
    let stem = if stem.is_empty() { "Field" } else { stem };
    (1..=taken.len() + 1)
        .map(|number| format!("{stem}{number}"))
        .find(|name| !taken.contains(name))
        .unwrap_or_else(|| format!("{stem}{}", taken.len() + 1))
}

pub(crate) fn plan_copy_fields(
    source: &ByteStore,
    page: PlannerPage<'_>,
    page_index: usize,
    copies: &[(Reference, [f64; 4])],
) -> Result<Plan, SpikeError> {
    let credential = page.credential;
    let originals = crate::form::fields_of_page(source, page.program.page, credential)?;
    let writes = in_sequence((source, credential), copies, |document, (widget, rect)| {
        let original = originals
            .iter()
            .find(|field| field.widget == *widget)
            .ok_or_else(|| refused("this is not a field of the document's form"))?;
        let kind = copied_kind(original)?;
        let taken = crate::form::field_names(document, credential);
        let name = copy_name(original, &taken);
        let widget_number = crate::block_rewrite::next_object_number(document)?;
        let placed = crate::new_field::plan_new_field(
            document,
            page,
            page_index,
            &NewField {
                rect: *rect,
                kind,
                name: Some(&name),
                options: &original.options,
            },
        )?;
        let mut writes = placed.writes().to_vec();
        let copy = Reference::new(widget_number, 0);
        let looks = looks_of(original);
        if !looks.is_empty() {
            if let Some(held) = writes.iter_mut().find(|held| held.reference == copy) {
                *held = crate::fill_field::set_entries_in(held, &looks)?;
            } else {
                let settled = crate::block_rewrite::commit_writes(
                    document,
                    &writes,
                    (credential, crate::Restrictions::SetAside),
                )?;
                writes.push(set_entries((&settled, credential), copy, &looks)?);
            }
        }
        Ok(writes)
    })?;
    Ok(plan_of(
        &page,
        page_index,
        writes,
        union(copies.iter().map(|(_, rect)| *rect)),
    ))
}

fn looks_of(field: &FormField) -> Vec<Entry> {
    let colour = |parts: [f64; 3]| format!("[{} {} {}]", parts[0], parts[1], parts[2]);
    let border = field
        .border
        .map_or_else(String::new, |border| format!(" /BC {}", colour(border)));
    let fill = field
        .background
        .map_or_else(String::new, |fill| format!(" /BG {}", colour(fill)));
    let looks = format!("<<{border}{fill} >>");
    let mut entries: Vec<Entry> = vec![(b"/MK", looks)];
    if field.kind != FieldKind::Radio {
        entries.push((b"/Ff", field.flags.to_string()));
    }
    if matches!(field.kind, FieldKind::Text | FieldKind::Combo) {
        entries.push((b"/Q", (field.quadding as u8).to_string()));
        entries.push((
            b"/DA",
            format!(
                "({})",
                String::from_utf8_lossy(&field.appearance)
                    .replace('\\', "\\\\")
                    .replace('(', "\\(")
                    .replace(')', "\\)")
            ),
        ));
        if let Some(most) = field.max_len {
            entries.push((b"/MaxLen", most.to_string()));
        }
    }
    entries
}

#[cfg(test)]
#[expect(
    clippy::float_cmp,
    reason = "a box written as numbers is read back as the same numbers"
)]
mod tests {
    use pdf_bytes::{ByteStore, SourceId};
    use pdf_syntax::Reference;

    use crate::form::{FieldKind, FieldValue, FormField, Quadding, fields_of_page};
    use crate::new_field::NewFieldKind;
    use crate::plan::{Command, Plan};
    use crate::spike_move_text::{SpikeError, plan_command_with_fonts, read_page};

    fn document(objects: &[&str]) -> ByteStore {
        let mut bytes = b"%PDF-1.7\n".to_vec();
        let mut offsets = Vec::new();
        for (index, object) in objects.iter().enumerate() {
            offsets.push(bytes.len());
            bytes.extend_from_slice(format!("{} 0 obj\n{object}\nendobj\n", index + 1).as_bytes());
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
        ByteStore::new(SourceId::new(1), bytes)
    }

    fn planned(source: &ByteStore, command: &Command) -> Result<Plan, SpikeError> {
        plan_command_with_fonts(
            source,
            command,
            b"",
            Some(crate::new_text::tests::provider()),
        )
    }

    fn after(source: &ByteStore, command: &Command) -> Result<ByteStore, SpikeError> {
        let plan = planned(source, command)?;
        crate::block_rewrite::commit_writes(
            source,
            plan.writes(),
            (b"", crate::Restrictions::Respect),
        )
    }

    fn fields(source: &ByteStore) -> Vec<FormField> {
        let page = read_page(source, 0, b"", None).expect("reads").program.page;
        fields_of_page(source, page, b"").expect("fields")
    }

    fn add(
        source: &ByteStore,
        kind: NewFieldKind,
        rect: [f64; 4],
        name: Option<&str>,
    ) -> ByteStore {
        after(
            source,
            &Command::AddField {
                page_index: 0,
                rect,
                kind,
                name: name.map(str::to_owned),
                options: Vec::new(),
            },
        )
        .expect("added")
    }

    fn a_form() -> ByteStore {
        let blank = document(&[
            "<< /Type /Catalog /Pages 2 0 R >>",
            "<< /Type /Pages /MediaBox [0 0 400 400] /Kids [3 0 R] /Count 1 >>",
            "<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << >> >>",
            "<< /Length 0 >>\nstream\n\nendstream",
        ]);
        let one = add(
            &blank,
            NewFieldKind::Text,
            [20.0, 300.0, 120.0, 320.0],
            None,
        );
        let two = add(&one, NewFieldKind::Text, [40.0, 250.0, 140.0, 270.0], None);
        let three = add(&two, NewFieldKind::Text, [30.0, 200.0, 130.0, 220.0], None);
        let four = add(
            &three,
            NewFieldKind::Checkbox,
            [200.0, 300.0, 214.0, 314.0],
            None,
        );
        let widget = fields(&four)[0].widget;
        after(
            &four,
            &Command::FillField {
                page_index: 0,
                widget,
                value: FieldValue::Text("AB".to_owned()),
            },
        )
        .expect("filled")
    }

    #[test]
    fn a_group_is_aligned_in_one_change_and_undone_in_one() {
        let source = a_form();
        let listed = fields(&source);
        let boxes: Vec<(Reference, [f64; 4])> = listed[..3]
            .iter()
            .map(|field| {
                let width = field.rect[2] - field.rect[0];
                (
                    field.widget,
                    [20.0, field.rect[1], 20.0 + width, field.rect[3]],
                )
            })
            .collect();
        let command = Command::SetFieldBoxes {
            page_index: 0,
            boxes,
        };
        let plan = planned(&source, &command).expect("planned");
        let aligned = crate::block_rewrite::commit_writes(
            &source,
            plan.writes(),
            (b"", crate::Restrictions::Respect),
        )
        .expect("aligned");
        let lefts: Vec<f64> = fields(&aligned)[..3]
            .iter()
            .map(|field| field.rect[0])
            .collect();
        assert_eq!(lefts, [20.0, 20.0, 20.0]);
        assert_eq!(fields(&aligned)[0].value, FieldValue::Text("AB".to_owned()));
        let back = plan.inverse(&source, b"").expect("an inverse");
        let undone = crate::block_rewrite::commit_writes(
            &aligned,
            back.writes(),
            (b"", crate::Restrictions::Respect),
        )
        .expect("undone");
        let lefts: Vec<f64> = fields(&undone)[..3]
            .iter()
            .map(|field| field.rect[0])
            .collect();
        assert_eq!(lefts, [20.0, 40.0, 30.0]);
    }

    #[test]
    fn fields_sized_to_match_redraw_what_they_hold() {
        let source = a_form();
        let listed = fields(&source);
        let boxes = vec![
            (listed[0].widget, [20.0, 300.0, 220.0, 330.0]),
            (listed[1].widget, [40.0, 250.0, 240.0, 280.0]),
        ];
        let resized = after(
            &source,
            &Command::SetFieldBoxes {
                page_index: 0,
                boxes,
            },
        )
        .expect("resized");
        let back = fields(&resized);
        assert_eq!(back[0].rect, [20.0, 300.0, 220.0, 330.0]);
        assert_eq!(back[1].rect, [40.0, 250.0, 240.0, 280.0]);
        assert_eq!(back[0].value, FieldValue::Text("AB".to_owned()));
    }

    #[test]
    fn a_field_named_twice_is_refused() {
        let source = a_form();
        let widget = fields(&source)[0].widget;
        let command = Command::SetFieldBoxes {
            page_index: 0,
            boxes: vec![
                (widget, [0.0, 0.0, 50.0, 20.0]),
                (widget, [10.0, 0.0, 60.0, 20.0]),
            ],
        };
        assert!(planned(&source, &command).is_err());
    }

    #[test]
    fn a_copy_is_the_same_field_under_the_next_name() {
        let source = a_form();
        let original = fields(&source)[0].clone();
        let set = after(
            &source,
            &Command::SetFieldSettings {
                page_index: 0,
                widget: original.widget,
                settings: crate::field_settings::FieldSettings {
                    required: Some(true),
                    quadding: Some(Quadding::Centre),
                    ..Default::default()
                },
            },
        )
        .expect("set");
        let copied = after(
            &set,
            &Command::CopyFields {
                page_index: 0,
                copies: vec![(original.widget, [20.0, 100.0, 120.0, 120.0])],
            },
        )
        .expect("copied");
        let listed = fields(&copied);
        let copy = listed.last().expect("the copy");
        assert_eq!(copy.kind, FieldKind::Text);
        assert_eq!(copy.name, "Text4");
        assert_eq!(copy.rect, [20.0, 100.0, 120.0, 120.0]);
        assert!(copy.required);
        assert_eq!(copy.quadding, Quadding::Centre);
        assert_eq!(copy.value, FieldValue::Empty);
        assert_eq!(copy.border, original.border);
    }

    #[test]
    fn two_copies_in_one_change_have_two_names() {
        let source = a_form();
        let listed = fields(&source);
        let copied = after(
            &source,
            &Command::CopyFields {
                page_index: 0,
                copies: vec![
                    (listed[0].widget, [20.0, 100.0, 120.0, 120.0]),
                    (listed[3].widget, [200.0, 100.0, 214.0, 114.0]),
                ],
            },
        )
        .expect("copied");
        let names: Vec<String> = fields(&copied)
            .into_iter()
            .map(|field| field.name)
            .collect();
        assert_eq!(
            names,
            ["Text1", "Text2", "Text3", "Check1", "Text4", "Check2"]
        );
    }

    #[test]
    fn a_copied_radio_button_joins_its_group() {
        let source = a_form();
        let radio = add(
            &source,
            NewFieldKind::Radio,
            [300.0, 300.0, 314.0, 314.0],
            Some("Size"),
        );
        let widget = fields(&radio).last().expect("the button").widget;
        let copied = after(
            &radio,
            &Command::CopyFields {
                page_index: 0,
                copies: vec![(widget, [330.0, 300.0, 344.0, 314.0])],
            },
        )
        .expect("copied");
        let buttons: Vec<FormField> = fields(&copied)
            .into_iter()
            .filter(|field| field.kind == FieldKind::Radio)
            .collect();
        assert_eq!(buttons.len(), 2);
        assert_eq!(buttons[0].field, buttons[1].field);
        assert_ne!(buttons[0].states, buttons[1].states);
    }

    #[test]
    fn several_fields_are_deleted_together() {
        let source = a_form();
        let listed = fields(&source);
        let removed = after(
            &source,
            &Command::RemoveFields {
                page_index: 0,
                widgets: vec![listed[0].widget, listed[2].widget],
            },
        )
        .expect("removed");
        let names: Vec<String> = fields(&removed)
            .into_iter()
            .map(|field| field.name)
            .collect();
        assert_eq!(names, ["Text2", "Check1"]);
    }
}
