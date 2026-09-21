use crate::recent::Ago;

use super::Lang;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Home {
    BlankDocument,
    BlankDocumentHelp,
    Untitled,
    OpenFile,
    PagesFromFile,
    PictureFromFile,
    OpenFileHelp,
    Recent,
    NothingRecent,
    ContinueAt(usize),
    WorkedOn(Ago),
    Missing,
    RemoveFromList,
    BackToDocument,
    UpOneFolder,
    HomeFolder,
    NoPdfHere,
    NoPictureHere,
    FolderUnreadable(String),
    Cancel,
    OpenChosen,
    ChooseInTheFileWindow,
    SaveACopy,
    SaveNewDocument,
    PagesToFile,
    SplitToFiles,
    PagesToPictures,
    PicturesToPages,
    PicturesToInsert,
    UsePictures(usize),
    FileName,
    SaveHere,
    NameTaken,
    NameIsNotOne,
}

impl Home {
    #[must_use]
    pub fn say(&self, lang: Lang) -> String {
        match lang {
            Lang::English => self.english(),
        }
    }

    fn english(&self) -> String {
        match self {
            Self::BlankDocument => "Blank document".to_owned(),
            Self::BlankDocumentHelp => "Start with one blank A4 page".to_owned(),
            Self::Untitled => "Untitled".to_owned(),
            Self::OpenFile => "Open a PDF".to_owned(),
            Self::PagesFromFile => "Choose a PDF to insert its pages".to_owned(),
            Self::PictureFromFile => "Choose pictures to place".to_owned(),
            Self::OpenFileHelp => "Choose a file on this computer".to_owned(),
            Self::Recent => "Recent".to_owned(),
            Self::NothingRecent => "Documents you open will be listed here.".to_owned(),
            Self::ContinueAt(page) => format!("Continue on page {page}"),
            Self::WorkedOn(ago) => match ago {
                Ago::JustNow => "Just now".to_owned(),
                Ago::Minutes(1) => "1 minute ago".to_owned(),
                Ago::Minutes(n) => format!("{n} minutes ago"),
                Ago::Hours(1) => "1 hour ago".to_owned(),
                Ago::Hours(n) => format!("{n} hours ago"),
                Ago::Yesterday => "Yesterday".to_owned(),
                Ago::Days(n) => format!("{n} days ago"),
                Ago::Long => "Over a month ago".to_owned(),
            },
            Self::Missing => "Moved or deleted".to_owned(),
            Self::RemoveFromList => "Remove from list".to_owned(),
            Self::BackToDocument => "Back to the document".to_owned(),
            Self::UpOneFolder => "Up one folder".to_owned(),
            Self::HomeFolder => "Home folder".to_owned(),
            Self::NoPdfHere => "No folders or PDF files here".to_owned(),
            Self::NoPictureHere => "No folders or pictures (JPEG, PNG) here".to_owned(),
            Self::FolderUnreadable(folder) => format!("{folder} cannot be read"),
            Self::Cancel => "Cancel".to_owned(),
            Self::OpenChosen => "Open".to_owned(),
            Self::ChooseInTheFileWindow => "Choose the file in the window that opened".to_owned(),
            Self::SaveACopy => "Save a copy as".to_owned(),
            Self::SaveNewDocument => "Save this document as".to_owned(),
            Self::PagesToFile => "Save the chosen pages as a new PDF".to_owned(),
            Self::SplitToFiles => "Split into PDFs, named after".to_owned(),
            Self::PagesToPictures => "Write the pages out as pictures, named after".to_owned(),
            Self::PicturesToPages => "Choose the pictures to make a PDF of".to_owned(),
            Self::PicturesToInsert => "Choose the pictures to put in as pages".to_owned(),
            Self::UsePictures(0) => "Use these".to_owned(),
            Self::UsePictures(1) => "Use this 1".to_owned(),
            Self::UsePictures(many) => format!("Use these {many}"),
            Self::FileName => "File name".to_owned(),
            Self::SaveHere => "Save".to_owned(),
            Self::NameTaken => "A file of that name is already in this folder".to_owned(),
            Self::NameIsNotOne => "Type one file name, not a path".to_owned(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Home, Lang};
    use crate::recent::Ago;

    #[test]
    fn ages_are_counted_in_english_and_thai() {
        let said = |ago| Home::WorkedOn(ago).say(Lang::English);
        assert_eq!(said(Ago::Minutes(1)), "1 minute ago");
        assert_eq!(said(Ago::Minutes(5)), "5 minutes ago");
        assert_eq!(said(Ago::Hours(1)), "1 hour ago");
        assert_eq!(said(Ago::Days(3)), "3 days ago");
        assert_eq!(
            Home::ContinueAt(261).say(Lang::English),
            "Continue on page 261"
        );
    }
}
