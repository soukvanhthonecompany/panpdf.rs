use eframe::egui;

use pdf_app::wording::{Fact, Lang, Message};
use pdf_edit::info::{self, DocumentInfo, Protection, Stamp};
use pdf_edit::reprotect;

use crate::window_state::Window;

const WIDTH: f32 = 520.0;

const HEIGHT: f32 = 448.0;

pub(crate) struct Properties {
    pub(crate) was: DocumentInfo,
    pub(crate) title: String,
    pub(crate) author: String,
    pub(crate) subject: String,
    pub(crate) keywords: String,
    pub(crate) protection: Option<Protection>,
    pub(crate) signatures: Vec<pdf_edit::signature::Signature>,
    pub(crate) file: FileFacts,
    pub(crate) half: Half,
    pub(crate) security: Security,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Half {
    General,
    Details,
    Security,
}

#[expect(
    clippy::struct_excessive_bools,
    reason = "each is one independent box on the page, and a person ticks them independently"
)]
pub(crate) struct Security {
    pub(crate) protect: bool,
    pub(crate) ask_to_open: bool,
    pub(crate) user: String,
    pub(crate) user_again: String,
    pub(crate) restrict: bool,
    pub(crate) owner: String,
    pub(crate) owner_again: String,
    pub(crate) allowed: reprotect::Allowed,
    pub(crate) shown: bool,
}

impl Security {
    fn of(protection: Option<Protection>) -> Self {
        let allowed = protection.map_or_else(reprotect::Allowed::default, |protection| {
            reprotect::Allowed {
                print: protection.may.print,
                modify: protection.may.modify,
                copy: protection.may.copy,
                annotate: protection.may.annotate,
                fill_forms: protection.may.fill_forms,
                assemble: protection.may.assemble,
            }
        });
        Self {
            protect: protection.is_some(),
            ask_to_open: false,
            user: String::new(),
            user_again: String::new(),
            restrict: protection
                .is_some_and(|protection| protection.may != info::Permissions::all()),
            owner: String::new(),
            owner_again: String::new(),
            allowed,
            shown: false,
        }
    }

    fn pair(typed: &str, again: &str) -> Result<Vec<u8>, bool> {
        if typed != again {
            return Err(true);
        }
        if typed.is_empty() {
            return Err(false);
        }
        Ok(typed.as_bytes().to_vec())
    }

    pub(crate) fn asked(&self, protection: Option<Protection>) -> Option<reprotect::Wanted> {
        if !self.protect {
            return protection.is_some().then_some(reprotect::Wanted::Open);
        }
        if !self.ask_to_open && !self.restrict {
            return None;
        }
        let user = if self.ask_to_open {
            Self::pair(&self.user, &self.user_again).ok()?
        } else {
            Vec::new()
        };
        let owner = if self.restrict {
            Self::pair(&self.owner, &self.owner_again).ok()?
        } else {
            Vec::new()
        };
        Some(reprotect::Wanted::Protected(Box::new(asked_of_the_engine(
            user,
            owner,
            self.restrict,
            self.allowed,
        ))))
    }
}

fn asked_of_the_engine(
    user: Vec<u8>,
    owner: Vec<u8>,
    restrict: bool,
    allowed: reprotect::Allowed,
) -> reprotect::Asked {
    reprotect::Asked {
        user,
        owner,
        allowed: if restrict {
            allowed
        } else {
            reprotect::Allowed::default()
        },
    }
}

pub(crate) struct FileFacts {
    version: String,
    bytes: u64,
    revisions: usize,
    objects: (usize, usize, usize),
    repairs: Vec<String>,
}

impl Window {
    pub(crate) fn open_the_properties(&mut self) {
        if self.properties.is_some() || !self.has_document() || self.home {
            return;
        }
        let Some(facts) = self.editor.facts() else {
            return;
        };
        let info = facts.info;
        self.properties = Some(Properties {
            title: info.title.clone(),
            author: info.author.clone(),
            subject: info.subject.clone(),
            keywords: info.keywords.clone(),
            was: info,
            protection: facts.protection,
            file: self.file_facts(),
            signatures: self
                .editor
                .source()
                .map(|source| {
                    pdf_edit::signature::signatures(source, self.editor.credential())
                        .unwrap_or_default()
                })
                .unwrap_or_default(),
            half: Half::General,
            security: Security::of(facts.protection),
        });
    }

    pub(crate) fn read_the_properties_key(&mut self, ctx: &egui::Context) {
        if self.leaving.is_some()
            || self.loading.is_some()
            || self.chooser.is_some()
            || self.asks_for_a_password()
        {
            return;
        }
        if ctx.input(|input| input.modifiers.command && input.key_pressed(egui::Key::D)) {
            self.open_the_properties();
        }
    }

    fn file_facts(&self) -> FileFacts {
        let Some(source) = self.editor.source() else {
            return FileFacts {
                version: String::new(),
                bytes: 0,
                revisions: 0,
                objects: (0, 0, 0),
                repairs: Vec::new(),
            };
        };
        let read = pdf_cli::inspect_recovering(source, pdf_cli::InspectLimits::default());
        let bytes = source.len() as u64;
        match read {
            Err(_) => FileFacts {
                version: String::new(),
                bytes,
                revisions: 0,
                objects: (0, 0, 0),
                repairs: Vec::new(),
            },
            Ok(report) => FileFacts {
                version: report.version.to_string(),
                bytes,
                revisions: report.revisions.len(),
                objects: (
                    report.objects.active(),
                    report.objects.compressed,
                    report.objects.free,
                ),
                repairs: report
                    .repairs
                    .iter()
                    .map(std::string::ToString::to_string)
                    .collect(),
            },
        }
    }

    pub(crate) fn properties_window(&mut self, ctx: &egui::Context) {
        if self.properties.is_none() {
            return;
        }
        let lang = self.lang;
        let working = !self.editor.is_busy();
        let (mut close, mut save) = (false, false);
        let mut apply = None;
        let unsaved = self.unsaved();
        let mut panel = self.properties.take().expect("the window is open");
        let heading = Fact::Properties.say(lang);
        let shown = egui::Modal::new(egui::Id::new("document-properties")).show(ctx, |ui| {
            ui.set_width(WIDTH);
            ui.heading(&heading);
            ui.add_space(2.0);
            ui.horizontal(|ui| {
                for (half, name) in [
                    (Half::General, Fact::General),
                    (Half::Details, Fact::Details),
                    (Half::Security, Fact::Security),
                ] {
                    ui.selectable_value(&mut panel.half, half, name.say(lang));
                }
            });
            ui.separator();
            egui::ScrollArea::vertical()
                .max_height(HEIGHT)
                .min_scrolled_height(HEIGHT)
                .auto_shrink([false, false])
                .show(ui, |ui| match panel.half {
                    Half::General => general_half(ui, &mut panel, lang),
                    Half::Details => self.details_half(ui, &panel),
                    Half::Security => security_half(ui, &mut panel, lang),
                });
            ui.separator();
            ui.horizontal(|ui| {
                match panel.half {
                    Half::General => {
                        let changed = panel.changed().is_some();
                        let button = egui::Button::new(Fact::Save.say(lang));
                        if ui.add_enabled(working && changed, button).clicked() {
                            save = true;
                        }
                        if !changed {
                            ui.label(
                                egui::RichText::new(Fact::NothingToSave.say(lang))
                                    .size(11.0)
                                    .color(ui.visuals().weak_text_color()),
                            );
                        }
                    }
                    Half::Security => {
                        let asked = panel.security.asked(panel.protection);
                        let ready = working && !unsaved && asked.is_some();
                        let button = egui::Button::new(Fact::Apply.say(lang));
                        if ui.add_enabled(ready, button).clicked() {
                            apply = asked;
                        }
                        if unsaved {
                            ui.label(
                                egui::RichText::new(Fact::SaveBeforeChangingProtection.say(lang))
                                    .size(11.0)
                                    .color(ui.visuals().weak_text_color()),
                            );
                        }
                    }
                    Half::Details => {}
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button(Fact::Close.say(lang)).clicked() {
                        close = true;
                    }
                });
            });
        });
        if shown.should_close() {
            close = true;
        }
        if save && let Some(edit) = panel.changed() {
            let job = self.editor.begin_describe(edit);
            self.send(job);
            close = true;
        }
        if let Some(wanted) = apply {
            self.change_the_protection(&wanted, &panel.security);
            close = true;
        }
        if !close {
            self.properties = Some(panel);
        }
    }

    fn details_half(&self, ui: &mut egui::Ui, panel: &Properties) {
        let lang = self.lang;
        let file = &panel.file;
        grid(ui, "properties-file", |ui| {
            if self.untitled {
                row(ui, &Fact::File.say(lang), &Fact::NoFileYet.say(lang), lang);
            } else {
                let name = self
                    .opened
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_default();
                row(ui, &Fact::File.say(lang), &name, lang);
                let folder = self
                    .opened
                    .parent()
                    .map(|folder| folder.to_string_lossy().into_owned())
                    .unwrap_or_default();
                row(ui, &Fact::Folder.say(lang), &folder, lang);
            }
            row(
                ui,
                &Fact::Size.say(lang),
                &Fact::Bytes(file.bytes).say(lang),
                lang,
            );
            row(ui, &Fact::Version.say(lang), &file.version, lang);
            row(
                ui,
                &Fact::Pages.say(lang),
                &self.editor.page_count().to_string(),
                lang,
            );
            if let Some(geometry) = self.editor.geometry(self.focus) {
                let (width, height) = geometry.rotated_size();
                row(
                    ui,
                    &Fact::PageSize.say(lang),
                    &Fact::Millimetres(width * 25.4 / 72.0, height * 25.4 / 72.0).say(lang),
                    lang,
                );
            }
        });
        ui.add_space(8.0);
        grid(ui, "properties-built", |ui| {
            let (active, compressed, free) = file.objects;
            row(
                ui,
                &Fact::Objects.say(lang),
                &format!("{active} ({compressed} + {free})"),
                lang,
            );
            row(
                ui,
                &Fact::Revisions.say(lang),
                &file.revisions.to_string(),
                lang,
            );
            if !file.repairs.is_empty() {
                row(ui, &Fact::Repairs.say(lang), &file.repairs.join("\n"), lang);
            }
        });
        ui.add_space(8.0);
        ui.label(egui::RichText::new(Fact::Signatures.say(lang)).strong());
        if panel.signatures.is_empty() {
            ui.label(
                egui::RichText::new(Fact::NoSignatures.say(lang))
                    .color(ui.visuals().weak_text_color()),
            );
        }
        for signature in &panel.signatures {
            signature_said(ui, signature, lang);
        }
        ui.add_space(8.0);
        ui.label(egui::RichText::new(Fact::Protection.say(lang)).strong());
        match panel.protection {
            None => {
                ui.label(Fact::NotProtected.say(lang));
            }
            Some(protection) => protection_said(ui, protection, lang),
        }
    }
}

fn protection_said(ui: &mut egui::Ui, protection: Protection, lang: Lang) {
    let owner = matches!(protection.access, info::AccessLevel::Owner);
    ui.label(format!(
        "{} · {}",
        Fact::Cipher(cipher_name(protection.stream_cipher)).say(lang),
        if owner {
            Fact::AsOwner.say(lang)
        } else {
            Fact::AsReader.say(lang)
        }
    ));
    ui.add_space(4.0);
    ui.label(
        egui::RichText::new(Fact::Allowed.say(lang))
            .size(11.0)
            .color(ui.visuals().weak_text_color()),
    );
    let may = protection.may;
    let printing = match may.print {
        info::PrintAllowance::Faithful => (Fact::MayPrint, true),
        info::PrintAllowance::Degraded => (Fact::MayPrintDegraded, true),
        info::PrintAllowance::Refused => (Fact::MayNotPrint, false),
    };
    for (what, allowed) in [
        printing,
        (Fact::MayEdit, may.modify),
        (Fact::MayCopy, may.copy),
        (Fact::MayAnnotate, may.annotate),
        (Fact::MayFillForms, may.fill_forms),
        (Fact::MayAssemble, may.assemble),
    ] {
        let mark = if allowed { '\u{2713}' } else { '\u{2715}' };
        let colour = if allowed {
            ui.visuals().text_color()
        } else {
            ui.visuals().weak_text_color()
        };
        ui.label(egui::RichText::new(format!("{mark}  {}", what.say(lang))).color(colour));
    }
}

const fn cipher_name(cipher: info::CipherMethod) -> &'static str {
    match cipher {
        info::CipherMethod::Identity => "No encryption of the content",
        info::CipherMethod::Rc4 => "RC4",
        info::CipherMethod::Aes128 => "AES-128",
        info::CipherMethod::Aes256 => "AES-256",
    }
}

fn general_half(ui: &mut egui::Ui, panel: &mut Properties, lang: Lang) {
    grid(ui, "properties-mine", |ui| {
        for (label, value) in [
            (Fact::Title, &mut panel.title),
            (Fact::Author, &mut panel.author),
            (Fact::Subject, &mut panel.subject),
            (Fact::Keywords, &mut panel.keywords),
        ] {
            ui.label(label.say(lang));
            ui.add(egui::TextEdit::singleline(value).desired_width(f32::INFINITY));
            ui.end_row();
        }
    });
    ui.add_space(2.0);
    ui.label(
        egui::RichText::new(Fact::YoursToChange.say(lang))
            .size(11.0)
            .color(ui.visuals().weak_text_color()),
    );
    ui.add_space(10.0);
    grid(ui, "properties-theirs", |ui| {
        row(ui, &Fact::MadeWith.say(lang), &panel.was.creator, lang);
        row(ui, &Fact::WrittenBy.say(lang), &panel.was.producer, lang);
        row(ui, &Fact::Created.say(lang), &said(panel.was.created), lang);
        row(
            ui,
            &Fact::Modified.say(lang),
            &said(panel.was.modified),
            lang,
        );
        for (key, value) in &panel.was.other {
            row(
                ui,
                &format!("{} · {key}", Fact::AlsoSaid.say(lang)),
                value,
                lang,
            );
        }
    });
    ui.add_space(6.0);
    ui.label(
        egui::RichText::new(Fact::DictionaryOnly.say(lang))
            .size(11.0)
            .color(ui.visuals().weak_text_color()),
    );
}

fn said(stamp: Option<Stamp>) -> String {
    let Some(stamp) = stamp else {
        return String::new();
    };
    let Stamp {
        year,
        month,
        day,
        hour,
        minute,
        offset_minutes,
        ..
    } = stamp;
    let when = format!("{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}");
    match offset_minutes {
        None => when,
        Some(0) => format!("{when} UTC"),
        Some(offset) => {
            let sign = if offset < 0 { '-' } else { '+' };
            format!(
                "{when} {sign}{:02}:{:02}",
                offset.abs() / 60,
                offset.abs() % 60
            )
        }
    }
}

fn row(ui: &mut egui::Ui, label: &str, value: &str, lang: Lang) {
    ui.label(egui::RichText::new(label).color(ui.visuals().weak_text_color()));
    if value.is_empty() {
        ui.label(
            egui::RichText::new(Fact::Unsaid.say(lang))
                .italics()
                .color(ui.visuals().weak_text_color()),
        );
    } else {
        ui.add(egui::Label::new(value).wrap());
    }
    ui.end_row();
}

fn grid(ui: &mut egui::Ui, id: &str, rows: impl FnOnce(&mut egui::Ui)) {
    egui::Grid::new(id)
        .num_columns(2)
        .spacing([12.0, 6.0])
        .min_col_width(110.0)
        .show(ui, rows);
}

impl Properties {
    pub(crate) fn changed(&self) -> Option<pdf_edit::info::InfoEdit> {
        let changed = |typed: &String, was: &String| {
            (typed.trim() != was.as_str()).then(|| typed.trim().to_owned())
        };
        let edit = pdf_edit::info::InfoEdit {
            title: changed(&self.title, &self.was.title),
            author: changed(&self.author, &self.was.author),
            subject: changed(&self.subject, &self.was.subject),
            keywords: changed(&self.keywords, &self.was.keywords),
            modified: Some(now()),
            created: self.was.created.is_none().then(now),
            ..pdf_edit::info::InfoEdit::default()
        };
        let typed = edit.title.is_some()
            || edit.author.is_some()
            || edit.subject.is_some()
            || edit.keywords.is_some();
        typed.then_some(edit)
    }
}

fn now() -> Stamp {
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_secs());
    let moment = pdf_app::dates::moment(seconds);
    Stamp {
        year: moment.year,
        month: moment.month,
        day: moment.day,
        hour: moment.hour,
        minute: moment.minute,
        second: moment.second,
        offset_minutes: Some(0),
    }
}

fn security_half(ui: &mut egui::Ui, panel: &mut Properties, lang: Lang) {
    let asking = &mut panel.security;
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.selectable_value(&mut asking.protect, false, Fact::NoProtection.say(lang));
        ui.selectable_value(
            &mut asking.protect,
            true,
            Fact::PasswordProtection.say(lang),
        );
    });
    ui.add_space(8.0);

    if !asking.protect {
        if panel.protection.is_some() {
            ui.label(Fact::WrittenWholeWarning.say(lang));
        } else {
            ui.label(
                egui::RichText::new(Fact::NotProtected.say(lang))
                    .color(ui.visuals().weak_text_color()),
            );
        }
        return;
    }

    ui.checkbox(&mut asking.ask_to_open, Fact::AskToOpen.say(lang));
    if asking.ask_to_open {
        ui.indent("open-password", |ui| {
            passwords(
                ui,
                (&mut asking.user, &mut asking.user_again),
                asking.shown,
                ("open", lang),
            );
        });
    }
    ui.add_space(6.0);

    ui.checkbox(&mut asking.restrict, Fact::RestrictWhatIsAllowed.say(lang));
    if asking.restrict {
        ui.indent("owner-password", |ui| {
            passwords(
                ui,
                (&mut asking.owner, &mut asking.owner_again),
                asking.shown,
                ("owner", lang),
            );
            ui.add_space(4.0);
            allowances(ui, &mut asking.allowed, lang);
        });
    }

    ui.add_space(6.0);
    ui.checkbox(&mut asking.shown, Message::ShowThePassword.say(lang));
    ui.add_space(8.0);
    if !asking.ask_to_open && !asking.restrict {
        ui.label(
            egui::RichText::new(Fact::ChooseOneOrTheOther.say(lang))
                .color(ui.visuals().weak_text_color()),
        );
        return;
    }
    ui.label(
        egui::RichText::new(Fact::WrittenWithAes.say(lang))
            .size(11.0)
            .color(ui.visuals().weak_text_color()),
    );
    ui.add_space(2.0);
    ui.label(
        egui::RichText::new(Fact::WrittenWholeWarning.say(lang))
            .size(11.0)
            .color(ui.visuals().weak_text_color()),
    );
}

fn passwords(
    ui: &mut egui::Ui,
    (typed, again): (&mut String, &mut String),
    shown: bool,
    (which, lang): (&str, Lang),
) {
    let differ = typed != again;
    grid(ui, &format!("password-{which}"), |ui| {
        ui.label(
            egui::RichText::new(Fact::PasswordBox.say(lang)).color(ui.visuals().weak_text_color()),
        );
        ui.add(
            egui::TextEdit::singleline(typed)
                .password(!shown)
                .desired_width(f32::INFINITY),
        );
        ui.end_row();
        ui.label(
            egui::RichText::new(Fact::TypeItAgain.say(lang)).color(ui.visuals().weak_text_color()),
        );
        ui.add(
            egui::TextEdit::singleline(again)
                .password(!shown)
                .desired_width(f32::INFINITY),
        );
        ui.end_row();
    });
    if differ {
        ui.label(
            egui::RichText::new(Fact::TheTwoDiffer.say(lang)).color(ui.visuals().error_fg_color),
        );
    }
}

fn allowances(ui: &mut egui::Ui, allowed: &mut reprotect::Allowed, lang: Lang) {
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new(Fact::PrintingAllowed.say(lang))
                .color(ui.visuals().weak_text_color()),
        );
        for (how, name) in [
            (info::PrintAllowance::Faithful, Fact::PrintFully),
            (info::PrintAllowance::Degraded, Fact::PrintLowOnly),
            (info::PrintAllowance::Refused, Fact::PrintNever),
        ] {
            ui.selectable_value(&mut allowed.print, how, name.say(lang));
        }
    });
    for (what, allow) in [
        (Fact::MayEdit, &mut allowed.modify),
        (Fact::MayCopy, &mut allowed.copy),
        (Fact::MayAnnotate, &mut allowed.annotate),
        (Fact::MayFillForms, &mut allowed.fill_forms),
        (Fact::MayAssemble, &mut allowed.assemble),
    ] {
        ui.checkbox(allow, what.say(lang));
    }
}

impl Window {
    pub(crate) fn change_the_protection(&mut self, wanted: &reprotect::Wanted, asking: &Security) {
        let Some(source) = self.editor.source().cloned() else {
            return;
        };
        let credential = self.editor.credential().to_vec();
        let opens = match wanted {
            reprotect::Wanted::Open => Vec::new(),
            reprotect::Wanted::Protected(_) if !asking.owner.is_empty() => {
                asking.owner.clone().into_bytes()
            }
            reprotect::Wanted::Protected(_) => asking.user.clone().into_bytes(),
        };
        let wanted = wanted.clone();
        let (path, page) = (self.opened.clone(), self.focus);
        self.editor
            .say(Message::Opening(crate::app::name_of(&path)));
        self.loading = Some(crate::window_state::Opening {
            path,
            page,
            changed_protection: true,
            tried_a_password: true,
            handle: std::thread::spawn(move || {
                match pdf_edit::reprotect::rewrite(&source, &credential, &wanted) {
                    Ok(bytes) => crate::chrome::open_bytes(
                        pdf_bytes::ByteStore::new(
                            pdf_bytes::SourceId::new(0),
                            std::sync::Arc::<[u8]>::from(bytes),
                        ),
                        &opens,
                    ),
                    Err(error) => crate::window_state::Opened::Refused(error.to_string()),
                }
            }),
        });
    }
}

fn signature_said(ui: &mut egui::Ui, signature: &pdf_edit::signature::Signature, lang: Lang) {
    use pdf_edit::signature::{Covers, Kind};

    ui.add_space(4.0);
    let kind = match signature.kind {
        Kind::Approval => Fact::SignedBy,
        Kind::Certification => Fact::CertifiedBy,
        Kind::Timestamp => Fact::TimestampedBy,
        Kind::UsageRights => Fact::RightsGrantedBy,
    };
    let who = if signature.name.is_empty() {
        Fact::Unsaid.say(lang)
    } else {
        signature.name.clone()
    };
    ui.label(format!("{} {who}", kind.say(lang)));
    grid(ui, &format!("signature-{}", signature.field), |ui| {
        if !signature.field.is_empty() {
            row(ui, &Fact::SignatureField.say(lang), &signature.field, lang);
        }
        let when = said(signature.signed);
        row(ui, &Fact::SignedOn.say(lang), &when, lang);
        if !signature.reason.is_empty() {
            row(ui, &Fact::SignedBecause.say(lang), &signature.reason, lang);
        }
        if !signature.location.is_empty() {
            row(ui, &Fact::SignedAt.say(lang), &signature.location, lang);
        }
        row(
            ui,
            &Fact::SignatureEncoding.say(lang),
            &signature.encoding,
            lang,
        );
    });
    match signature.covers {
        Covers::WholeDocument => {
            ui.label(format!("\u{2713}  {}", Fact::CoversTheWholeFile.say(lang)));
        }
        Covers::UpTo { signed_through, of } => {
            ui.label(
                egui::RichText::new(format!(
                    "\u{2715}  {}",
                    Fact::ChangedAfterSigning(of - signed_through).say(lang)
                ))
                .color(ui.visuals().warn_fg_color),
            );
        }
        Covers::Unstated => {
            ui.label(
                egui::RichText::new(Fact::CoverageUnstated.say(lang))
                    .color(ui.visuals().weak_text_color()),
            );
        }
    }
    ui.add_space(4.0);
    match &signature.checked {
        None => {}
        Some(checked) => verdict_said(ui, checked, lang),
    }
}

fn verdict_said(ui: &mut egui::Ui, checked: &pdf_edit::signature::Checked, lang: Lang) {
    use pdf_edit::signature::{Integrity, Trust};

    let (mark, fact, colour) = match &checked.integrity {
        Integrity::Intact => ('\u{2713}', Fact::SignatureIntact, ui.visuals().text_color()),
        Integrity::ContentChanged => (
            '\u{2715}',
            Fact::SignatureContentChanged,
            ui.visuals().error_fg_color,
        ),
        Integrity::SignatureWrong => (
            '\u{2715}',
            Fact::SignatureIsWrong,
            ui.visuals().error_fg_color,
        ),
        Integrity::CannotCheck(why) => (
            '?',
            Fact::SignatureCannotCheck(why.clone()),
            ui.visuals().weak_text_color(),
        ),
    };
    ui.label(egui::RichText::new(format!("{mark}  {}", fact.say(lang))).color(colour));

    if let Some(signer) = &checked.signer {
        ui.add_space(2.0);
        ui.label(Fact::CertificateFor(signer.common.clone()).say(lang));
        ui.label(
            egui::RichText::new(signer.full.clone())
                .size(11.0)
                .color(ui.visuals().weak_text_color()),
        );
        if let Some(issuer) = &checked.issuer {
            ui.label(
                egui::RichText::new(Fact::CertificateIssuedBy(issuer.full.clone()).say(lang))
                    .size(11.0)
                    .color(ui.visuals().weak_text_color()),
            );
        }
    }

    ui.add_space(2.0);
    let (mark, fact, colour) = match &checked.trust {
        Trust::Anchored { root } => (
            '\u{2713}',
            Fact::TrustedThrough(root.clone()),
            ui.visuals().text_color(),
        ),
        Trust::UnknownAuthority { top } => (
            '!',
            Fact::AuthorityNotKnown(top.clone()),
            ui.visuals().warn_fg_color,
        ),
        Trust::Incomplete => ('!', Fact::ChainIncomplete, ui.visuals().warn_fg_color),
        Trust::NoStore => (
            '!',
            Fact::NoListOfAuthorities,
            ui.visuals().weak_text_color(),
        ),
    };
    ui.label(egui::RichText::new(format!("{mark}  {}", fact.say(lang))).color(colour));

    let small = |ui: &mut egui::Ui, text: String| {
        ui.label(
            egui::RichText::new(text)
                .size(11.0)
                .color(ui.visuals().weak_text_color()),
        );
    };
    ui.add_space(2.0);
    if let (Some(hash), Some(bits)) = (checked.hash, checked.key_bits) {
        small(
            ui,
            Fact::SignatureMadeWith(format!("RSA {bits}, {hash}")).say(lang),
        );
    }
    if let Some(when) = checked.signed_at {
        small(ui, Fact::SignedAtMoment(when.write()).say(lang));
    }
    if let Some(hash) = checked.hash.filter(|_| !checked.hash_is_sound) {
        ui.label(
            egui::RichText::new(format!(
                "!  {}",
                Fact::HashNoLongerProves(hash.to_owned()).say(lang)
            ))
            .size(11.0)
            .color(ui.visuals().warn_fg_color),
        );
    }
    if let Some((from, until)) = checked
        .certificate_life
        .filter(|_| !checked.certificate_is_current)
    {
        ui.label(
            egui::RichText::new(format!(
                "!  {}",
                Fact::CertificateLife(from.write(), until.write()).say(lang)
            ))
            .size(11.0)
            .color(ui.visuals().warn_fg_color),
        );
    }
    small(ui, Fact::RevocationNotChecked.say(lang));
}
