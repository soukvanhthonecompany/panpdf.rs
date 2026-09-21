use super::Lang;

#[derive(Clone, Debug, PartialEq)]
pub enum Fact {
    Properties,
    General,
    Details,
    Security,
    Title,
    Author,
    Subject,
    Keywords,
    YoursToChange,
    Save,
    Close,
    NothingToSave,
    MadeWith,
    WrittenBy,
    Created,
    Modified,
    AlsoSaid,
    File,
    Folder,
    Size,
    Version,
    Pages,
    PageSize,
    NoFileYet,
    Objects,
    Revisions,
    Repairs,
    Signatures,
    NoSignatures,
    SignedBy,
    CertifiedBy,
    TimestampedBy,
    RightsGrantedBy,
    SignatureField,
    SignedOn,
    SignedBecause,
    SignedAt,
    SignatureEncoding,
    CoversTheWholeFile,
    ChangedAfterSigning(u64),
    CoverageUnstated,
    SignatureIntact,
    SignatureContentChanged,
    SignatureIsWrong,
    SignatureCannotCheck(String),
    CertificateFor(String),
    CertificateIssuedBy(String),
    SignatureMadeWith(String),
    TrustedThrough(String),
    AuthorityNotKnown(String),
    ChainIncomplete,
    NoListOfAuthorities,
    HashNoLongerProves(String),
    CertificateLife(String, String),
    SignedAtMoment(String),
    RevocationNotChecked,
    Protection,
    NotProtected,
    AsOwner,
    AsReader,
    Allowed,
    MayPrint,
    MayPrintDegraded,
    MayNotPrint,
    MayEdit,
    MayCopy,
    MayAnnotate,
    MayFillForms,
    MayAssemble,
    DictionaryOnly,
    NoProtection,
    PasswordProtection,
    AskToOpen,
    RestrictWhatIsAllowed,
    PasswordBox,
    TypeItAgain,
    TheTwoDiffer,
    ChooseOneOrTheOther,
    PrintingAllowed,
    PrintFully,
    PrintLowOnly,
    PrintNever,
    Apply,
    WrittenWholeWarning,
    SaveBeforeChangingProtection,
    ProtectionChanged,
    WrittenWithAes,
    Unsaid,
    Bytes(u64),
    Millimetres(f64, f64),
    Cipher(&'static str),
}

impl Fact {
    #[must_use]
    pub fn say(&self, lang: Lang) -> String {
        match lang {
            Lang::English => self.english(),
        }
    }

    #[expect(
        clippy::too_many_lines,
        reason = "one line per thing the panel can say, in the order it says them"
    )]
    fn english(&self) -> String {
        match self {
            Self::Properties => "Document properties".to_owned(),
            Self::General => "General".to_owned(),
            Self::Details => "Details".to_owned(),
            Self::Security => "Security".to_owned(),
            Self::Title => "Title".to_owned(),
            Self::Author => "Author".to_owned(),
            Self::Subject => "Subject".to_owned(),
            Self::Keywords => "Keywords".to_owned(),
            Self::YoursToChange => "These travel with the file: a search, a library and another \
                 reader all show them."
                .to_owned(),
            Self::Save => "Save".to_owned(),
            Self::Close => "Close".to_owned(),
            Self::NothingToSave => "Nothing changed".to_owned(),
            Self::MadeWith => "Made with".to_owned(),
            Self::WrittenBy => "PDF written by".to_owned(),
            Self::Created => "Created".to_owned(),
            Self::Modified => "Last changed".to_owned(),
            Self::AlsoSaid => "Also said".to_owned(),
            Self::File => "File".to_owned(),
            Self::Folder => "Folder".to_owned(),
            Self::Size => "Size".to_owned(),
            Self::Version => "PDF version".to_owned(),
            Self::Pages => "Pages".to_owned(),
            Self::PageSize => "Page size".to_owned(),
            Self::NoFileYet => "Not saved to a file yet".to_owned(),
            Self::Objects => "Objects".to_owned(),
            Self::Revisions => "Times written".to_owned(),
            Self::Repairs => "Read past".to_owned(),
            Self::Signatures => "Digital signatures".to_owned(),
            Self::NoSignatures => "None".to_owned(),
            Self::SignedBy => "Signed by".to_owned(),
            Self::CertifiedBy => "Certified by".to_owned(),
            Self::TimestampedBy => "Timestamped by".to_owned(),
            Self::RightsGrantedBy => "Reader rights granted by".to_owned(),
            Self::SignatureField => "On field".to_owned(),
            Self::SignedOn => "Dated".to_owned(),
            Self::SignedBecause => "Reason".to_owned(),
            Self::SignedAt => "Place".to_owned(),
            Self::SignatureEncoding => "Written as".to_owned(),
            Self::CoversTheWholeFile => "Covers the whole file".to_owned(),
            Self::ChangedAfterSigning(bytes) => format!(
                "{} were added to the file after this was signed, and it says nothing about them",
                bytes_said(*bytes)
            ),
            Self::CoverageUnstated => "It does not say which part of the file it covers".to_owned(),
            Self::SignatureIntact => {
                "The signature is intact: these bytes are the ones that were signed".to_owned()
            }
            Self::SignatureContentChanged => {
                "The part of the file this signature covers is not what was signed: it was \
                 altered after signing"
                    .to_owned()
            }
            Self::SignatureIsWrong => {
                "The signature is not the one this certificate's key makes".to_owned()
            }
            Self::SignatureCannotCheck(why) => format!("This could not be checked: {why}"),
            Self::CertificateFor(name) => format!("Certificate for {name}"),
            Self::CertificateIssuedBy(name) => format!("Issued by {name}"),
            Self::SignatureMadeWith(how) => format!("Made with {how}"),
            Self::TrustedThrough(root) => {
                format!("Signed under {root}, which this computer's list of authorities holds")
            }
            Self::AuthorityNotKnown(top) => format!(
                "Every certificate in the chain checks out, and it ends at {top}, whom this \
                 computer does not know. Nothing here says the name is anybody's real name."
            ),
            Self::ChainIncomplete => "The signature does not carry the certificates needed to \
                 follow it back to an authority"
                .to_owned(),
            Self::NoListOfAuthorities => "This computer keeps no list of authorities where one is \
                 looked for, so nothing was looked up"
                .to_owned(),
            Self::HashNoLongerProves(hash) => {
                format!("Signed with {hash}, which no longer establishes which document was signed")
            }
            Self::CertificateLife(from, until) => {
                format!("The certificate was valid from {from} until {until}, which is not now")
            }
            Self::SignedAtMoment(when) => format!("Signed at {when}, by the signer's own clock"),
            Self::RevocationNotChecked => "Whether the certificate was withdrawn is not checked: \
                 that needs the authority to be asked over the network"
                .to_owned(),
            Self::Protection => "Protection".to_owned(),
            Self::NotProtected => "Not protected".to_owned(),
            Self::AsOwner => "Opened as the owner".to_owned(),
            Self::AsReader => "Opened as a reader".to_owned(),
            Self::Allowed => "This document allows".to_owned(),
            Self::MayPrint => "Printing".to_owned(),
            Self::MayPrintDegraded => "Printing, at low quality only".to_owned(),
            Self::MayNotPrint => "No printing".to_owned(),
            Self::MayEdit => "Changing the content".to_owned(),
            Self::MayCopy => "Copying text out".to_owned(),
            Self::MayAnnotate => "Adding notes".to_owned(),
            Self::MayFillForms => "Filling in forms".to_owned(),
            Self::MayAssemble => "Putting pages in and out".to_owned(),
            Self::DictionaryOnly => {
                "Read from the document information dictionary. A file may also \
                 carry XMP metadata, which this does not show."
                    .to_owned()
            }
            Self::NoProtection => "No protection".to_owned(),
            Self::PasswordProtection => "Password protection".to_owned(),
            Self::AskToOpen => "Ask for a password to open the document".to_owned(),
            Self::RestrictWhatIsAllowed => "Restrict what the document allows".to_owned(),
            Self::PasswordBox => "Password".to_owned(),
            Self::TypeItAgain => "Type it again".to_owned(),
            Self::TheTwoDiffer => "The two boxes do not hold the same thing".to_owned(),
            Self::ChooseOneOrTheOther => {
                "Choose a password to open it, or a restriction, or both.".to_owned()
            }
            Self::PrintingAllowed => "Printing allowed".to_owned(),
            Self::PrintFully => "Full quality".to_owned(),
            Self::PrintLowOnly => "Low quality only".to_owned(),
            Self::PrintNever => "Not at all".to_owned(),
            Self::Apply => "Apply".to_owned(),
            Self::WrittenWholeWarning => "Changing this writes the whole file again, so the \
                 document's earlier revisions -- what Undo reaches -- are not kept. Nothing is \
                 written to disc until you save."
                .to_owned(),
            Self::SaveBeforeChangingProtection => {
                "Save this document before changing its protection.".to_owned()
            }
            Self::ProtectionChanged => "The protection was changed. Save to keep it.".to_owned(),
            Self::WrittenWithAes => "Written with AES-256.".to_owned(),
            Self::Unsaid => "Not said".to_owned(),
            Self::Bytes(bytes) => bytes_said(*bytes),
            Self::Millimetres(width, height) => format!("{width:.0} × {height:.0} mm"),
            Self::Cipher(name) => (*name).to_owned(),
        }
    }
}

pub(in crate::wording) fn bytes_said(bytes: u64) -> String {
    #[expect(
        clippy::cast_precision_loss,
        reason = "a size shown to one decimal place"
    )]
    let float = bytes as f64;
    match bytes {
        0..1024 => format!("{bytes} B"),
        1024..1_048_576 => format!("{:.1} KB", float / 1024.0),
        1_048_576..1_073_741_824 => format!("{:.1} MB", float / 1_048_576.0),
        _ => format!("{:.1} GB", float / 1_073_741_824.0),
    }
}
