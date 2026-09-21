use pdf_bytes::ByteStore;
use pdf_syntax::{ObjectKind, Reference};

use crate::object_edit::{ObjectEdit, reference_text};
use crate::plan::{Capability, Effect, Plan};
use crate::spike_move_text::{PlannerPage, SpikeError};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TabOrder {
    AsListed,
    Rows,
    Columns,
    Structure,
}

impl TabOrder {
    const fn name(self) -> Option<&'static str> {
        match self {
            Self::AsListed => None,
            Self::Rows => Some("R"),
            Self::Columns => Some("C"),
            Self::Structure => Some("S"),
        }
    }
}

fn refused(reason: &'static str) -> SpikeError {
    SpikeError::RetypeUnsupported(reason)
}

pub(crate) fn plan_set_tab_order(
    source: &ByteStore,
    page: PlannerPage<'_>,
    page_index: usize,
    (widgets, order): (&[Reference], TabOrder),
) -> Result<Plan, SpikeError> {
    let page_reference = page.program.page;
    let listed = crate::form::fields_of_page(source, page_reference, b"")?;
    for (at, widget) in widgets.iter().enumerate() {
        if widgets[..at].contains(widget) {
            return Err(refused("a field was named twice"));
        }
        if !listed.iter().any(|field| field.widget == *widget) {
            return Err(refused("this is not a field of this page"));
        }
    }

    let page_edit = ObjectEdit::of(source, page_reference)?;
    let dictionary = page_edit.value();
    let annots = crate::object_edit::entry(&page_edit.body, &dictionary, b"/Annots")
        .cloned()
        .ok_or_else(|| refused("this page lists no annotations to order"))?;
    let held = match annots.kind() {
        ObjectKind::Array(_) => None,
        ObjectKind::Reference(held) => Some(*held),
        _ => return Err(refused("this page's annotations cannot be read")),
    };
    let (array, array_edit) = match held {
        None => (annots.clone(), None),
        Some(held) => {
            let edit = ObjectEdit::of(source, held)?;
            (edit.value(), Some(edit))
        }
    };
    let ordered = reordered(&array, widgets)?;
    let tabs = order.name().map(|name| format!("/{name}"));
    let mut writes = Vec::new();
    if let Some(mut edit) = array_edit {
        {
            let whole = edit.value();
            edit.replace(&whole, &ordered)?;
            writes.push(edit.written()?);
            let mut page_edit = ObjectEdit::of(source, page_reference)?;
            let dictionary = page_edit.value();
            set_tabs(&mut page_edit, &dictionary, tabs.as_deref())?;
            writes.push(page_edit.written()?);
        }
    } else {
        {
            let mut edit = ObjectEdit::of(source, page_reference)?;
            let dictionary = edit.value();
            let annots = crate::object_edit::entry(&edit.body, &dictionary, b"/Annots")
                .cloned()
                .ok_or_else(|| refused("this page lists no annotations to order"))?;
            edit.replace(&annots, &ordered)?;
            set_tabs(&mut edit, &dictionary, tabs.as_deref())?;
            writes.push(edit.written()?);
        }
    }

    for write in &writes {
        if let crate::plan::PlannedBody::Direct { body } = &write.body {
            eprintln!(
                "TABDEBUG {} -> {}",
                write.reference.object_number(),
                String::from_utf8_lossy(body)
            );
        }
    }
    let document =
        crate::block_rewrite::commit_writes(source, &writes, crate::Restrictions::SetAside)?;
    let back: Vec<Reference> = crate::form::fields_of_page(&document, page_reference, b"")?
        .into_iter()
        .map(|field| field.widget)
        .collect();
    if back.len() < widgets.len() || back[..widgets.len()] != *widgets {
        return Err(refused(
            "the fields do not read back in the order asked for",
        ));
    }
    let target = page
        .program
        .streams
        .first()
        .map_or(page_reference, |stream| stream.reference);
    Ok(Plan::new(
        Capability::Exact,
        writes,
        Effect {
            page_index,
            moved: Vec::new(),
            target_stream: target,
            declared_region: None,
        },
    ))
}

fn set_tabs(
    edit: &mut ObjectEdit,
    dictionary: &pdf_syntax::Object,
    tabs: Option<&str>,
) -> Result<(), SpikeError> {
    match tabs {
        Some(name) => edit.set(dictionary, b"/Tabs", name),
        None => edit.unset(dictionary, b"/Tabs"),
    }
}

fn reordered(array: &pdf_syntax::Object, widgets: &[Reference]) -> Result<String, SpikeError> {
    let ObjectKind::Array(items) = array.kind() else {
        return Err(refused("this page's annotations cannot be read"));
    };
    let rest: Vec<Reference> = items
        .iter()
        .filter_map(|item| match item.kind() {
            ObjectKind::Reference(reference) if !widgets.contains(reference) => Some(*reference),
            _ => None,
        })
        .collect();
    if items.len() != rest.len() + widgets.len() {
        return Err(refused(
            "this page lists an annotation that is not an object of its own, so its order cannot be written",
        ));
    }
    let listed: Vec<String> = widgets
        .iter()
        .chain(&rest)
        .map(|reference| reference_text(*reference))
        .collect();
    Ok(format!("[{}]", listed.join(" ")))
}

#[must_use]
pub fn in_reading_order(fields: &[(Reference, [f64; 4])], columns: bool) -> Vec<Reference> {
    const ONE_ROW: f64 = 6.0;
    let mut order: Vec<&(Reference, [f64; 4])> = fields.iter().collect();
    order.sort_by(|one, other| {
        let (a, b) = (one.1, other.1);
        if columns {
            if (a[0] - b[0]).abs() <= ONE_ROW {
                b[3].total_cmp(&a[3])
            } else {
                a[0].total_cmp(&b[0])
            }
        } else if (a[3] - b[3]).abs() <= ONE_ROW {
            a[0].total_cmp(&b[0])
        } else {
            b[3].total_cmp(&a[3])
        }
    });
    order.into_iter().map(|(widget, _)| *widget).collect()
}

#[cfg(test)]
mod tests {
    use super::in_reading_order;
    use pdf_syntax::Reference;

    #[test]
    fn reading_order_runs_along_rows_or_down_columns() {
        let at = |number, rect| (Reference::new(number, 0), rect);
        let fields = [
            at(1, [200.0, 700.0, 300.0, 720.0]),
            at(2, [20.0, 700.0, 120.0, 722.0]),
            at(3, [20.0, 600.0, 120.0, 620.0]),
        ];
        let numbers = |order: Vec<Reference>| {
            order
                .into_iter()
                .map(Reference::object_number)
                .collect::<Vec<_>>()
        };
        assert_eq!(numbers(in_reading_order(&fields, false)), [2, 1, 3]);
        assert_eq!(numbers(in_reading_order(&fields, true)), [2, 3, 1]);
    }

    #[test]
    fn a_page_is_tabbed_through_in_the_order_asked_for() {
        use crate::form::fields_of_page;
        use crate::plan::Command;
        use crate::spike_move_text::{plan_command_with_fonts, read_page};

        let blank = crate::new_field::tests::document(&[
            "<< /Type /Catalog /Pages 2 0 R >>",
            "<< /Type /Pages /MediaBox [0 0 400 400] /Kids [3 0 R] /Count 1 >>",
            "<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << >> >>",
            "<< /Length 0 >>\nstream\n\nendstream",
        ]);
        let add = |source: &pdf_bytes::ByteStore, rect: [f64; 4]| {
            let command = Command::AddField {
                page_index: 0,
                rect,
                kind: crate::new_field::NewFieldKind::Text,
                name: None,
                options: Vec::new(),
            };
            let plan = plan_command_with_fonts(source, &command, b"", None).expect("planned");
            crate::block_rewrite::commit_writes(source, plan.writes(), crate::Restrictions::Respect)
                .expect("added")
        };
        let one = add(&blank, [20.0, 300.0, 120.0, 320.0]);
        let two = add(&one, [200.0, 300.0, 300.0, 320.0]);
        let three = add(&two, [20.0, 200.0, 120.0, 220.0]);
        let page = read_page(&three, 0, b"", None).expect("reads").program.page;
        let listed: Vec<Reference> = fields_of_page(&three, page, b"")
            .expect("fields")
            .into_iter()
            .map(|field| field.widget)
            .collect();
        let wanted = vec![listed[2], listed[0], listed[1]];
        let command = Command::SetTabOrder {
            page_index: 0,
            widgets: wanted.clone(),
            order: super::TabOrder::AsListed,
        };
        let plan = plan_command_with_fonts(&three, &command, b"", None).expect("planned");
        let ordered = crate::block_rewrite::commit_writes(
            &three,
            plan.writes(),
            crate::Restrictions::Respect,
        )
        .expect("ordered");
        let back: Vec<Reference> = fields_of_page(&ordered, page, b"")
            .expect("fields")
            .into_iter()
            .map(|field| field.widget)
            .collect();
        assert_eq!(back, wanted);
        for widgets in [vec![listed[0], listed[0]], vec![Reference::new(9_999, 0)]] {
            let command = Command::SetTabOrder {
                page_index: 0,
                widgets,
                order: super::TabOrder::AsListed,
            };
            assert!(plan_command_with_fonts(&three, &command, b"", None).is_err());
        }
        let command = Command::SetTabOrder {
            page_index: 0,
            widgets: wanted,
            order: super::TabOrder::Rows,
        };
        let plan = plan_command_with_fonts(&ordered, &command, b"", None).expect("planned");
        let rows = crate::block_rewrite::commit_writes(
            &ordered,
            plan.writes(),
            crate::Restrictions::Respect,
        )
        .expect("rows");
        assert!(String::from_utf8_lossy(rows.as_bytes()).contains("/Tabs /R"));
    }

    #[test]
    fn a_page_with_an_annotation_of_no_object_is_refused() {
        use crate::form::fields_of_page;
        use crate::plan::Command;
        use crate::spike_move_text::{plan_command_with_fonts, read_page};

        let source = crate::new_field::tests::document(&[
            "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [5 0 R] >> >>",
            "<< /Type /Pages /MediaBox [0 0 400 400] /Kids [3 0 R] /Count 1 >>",
            "<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << >> \
             /Annots [5 0 R << /Type /Annot /Subtype /Square /Rect [0 0 10 10] >>] >>",
            "<< /Length 0 >>\nstream\n\nendstream",
            "<< /Type /Annot /Subtype /Widget /FT /Tx /T (Name) /Rect [20 300 120 320] >>",
        ]);
        let page = read_page(&source, 0, b"", None)
            .expect("reads")
            .program
            .page;
        let widgets: Vec<Reference> = fields_of_page(&source, page, b"")
            .expect("fields")
            .into_iter()
            .map(|field| field.widget)
            .collect();
        let command = Command::SetTabOrder {
            page_index: 0,
            widgets,
            order: super::TabOrder::AsListed,
        };
        assert!(plan_command_with_fonts(&source, &command, b"", None).is_err());
    }
}
