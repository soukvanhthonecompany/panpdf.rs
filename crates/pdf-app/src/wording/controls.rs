use super::Lang;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Control {
    RecentColours,
    CustomColour,
    UseThisColour,
    FontSize,
    PlaceField(String),
    FieldPurpose(pdf_edit::new_field::NewFieldKind),
    LinkChosen,
    FieldSettings,
}

impl Control {
    #[must_use]
    pub fn say(&self, lang: Lang) -> String {
        match lang {
            Lang::English => self.english(),
        }
    }

    fn english(&self) -> String {
        match self {
            Self::RecentColours => "Recent".to_owned(),
            Self::CustomColour => "Your own colour".to_owned(),
            Self::UseThisColour => "Use this colour".to_owned(),
            Self::FontSize => "Font size \u{b7} type a size or pick one".to_owned(),
            Self::PlaceField(kind) => {
                format!("Click the page to put a {kind} field there, or drag to size it")
            }
            Self::FieldPurpose(kind) => english_purpose(*kind).to_owned(),
            Self::LinkChosen => "Drag to move \u{b7} Delete removes it".to_owned(),
            Self::FieldSettings => {
                "Set up the next field: its caption, choices or group".to_owned()
            }
        }
    }
}

fn english_purpose(kind: pdf_edit::new_field::NewFieldKind) -> &'static str {
    use pdf_edit::new_field::NewFieldKind as Kind;
    match kind {
        Kind::Text => "One line to type in: a name, a number",
        Kind::Paragraph => "Several lines to type in",
        Kind::Checkbox => "A box to tick: yes or no",
        Kind::Radio => "Pick one of several: put one per choice, in the same group",
        Kind::Dropdown => "A list that drops down to pick from",
        Kind::ListBox => "A list shown open to pick from",
        Kind::Date => "A date",
        Kind::Signature => "A place to sign",
        Kind::Button => "A button to press",
    }
}
