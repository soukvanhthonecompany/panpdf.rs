use std::fmt;

use super::{Lang, SEP};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Side {
    Left,
    Top,
    Right,
    Bottom,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Hidden {
    #[default]
    Nothing,
    Part,
    All,
}

impl Hidden {
    fn english(self) -> &'static str {
        match self {
            Self::Nothing => "",
            Self::Part => " \u{b7} part of it is behind a crop this file puts over the page",
            Self::All => {
                " \u{b7} all of it is behind a crop this file puts over the page; its frame is \
                 still there to drag it back, and Undo restores it"
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Done {
    MovedRun {
        capability: String,
        hidden: Hidden,
    },
    MovedBlock {
        commands: usize,
        capability: String,
        hidden: Hidden,
    },
    MovedGroup {
        pieces: usize,
    },
    MovedPicture {
        capability: String,
        hidden: Hidden,
    },
    ShapedPicture {
        capability: String,
    },
    ShapedParagraph {
        capability: String,
    },
    Sized {
        points: f64,
        capability: String,
    },
    Turned {
        degrees: f64,
        capability: String,
    },
    Slanted {
        degrees: f64,
        capability: String,
    },
    AnglesUnchanged {
        capability: String,
    },
    DeletedBlock {
        glyphs: usize,
        capability: String,
    },
    SetInReadableFace {
        family: String,
    },
    RemovedPicture {
        capability: String,
    },
    AddedText {
        characters: usize,
    },
    AddedPicture,
    AddedPictures {
        count: usize,
    },
    DrewLine,
    DrewShape,
    FilledField,
    AddedField,
    RemovedField,
    MovedField,
    ChangedField,
    MovedFields {
        count: usize,
    },
    CopiedFields {
        count: usize,
    },
    ChangedBookmarks,
    AddedLink,
    ChangedLink,
    MovedLink,
    RemovedLink,
    ChangedLinkLook,
    NamedAPlace,
    RenamedAPlace,
    RemovedAPlace,
    AddedLinks {
        count: usize,
    },
    MovedLinks {
        count: usize,
    },
    RemovedLinks {
        count: usize,
    },
    OrderedFields,
    RemovedFields {
        count: usize,
    },
    ClearedField,
    MarkedThePage,
    DeletedDrawing,
    AddedPage {
        page: usize,
    },
    Stamped {
        count: usize,
    },
    Recognized {
        pages: usize,
        confidence: u8,
        had_text: usize,
        refused: usize,
    },
    NothingToRecognize,
    RecognitionStopped,
    RecognitionOutdated,
    Described,
    RotatedPages {
        count: usize,
    },
    RemovedPages {
        count: usize,
    },
    MovedPages {
        count: usize,
        to: usize,
    },
    InsertedPages {
        count: usize,
        at: usize,
    },
    Undone,
    Redone,
    NothingToUndo,
    NothingToRedo,
    NothingToWalk,
    FrameRestored,
    NothingChanged,
    NothingToDeleteHere,
    Styled,
    Typed {
        deleted: bool,
        layout: Layout,
        overflow: bool,
        brought_in: Option<String>,
        drawn_from: Option<String>,
        cropped: bool,
    },
    DeletedOnRow {
        clusters: usize,
        gap_closed: bool,
        not_laid_out: Box<Refusal>,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub enum Layout {
    InFrame,
    OnItsRow(Box<Refusal>),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Refusal {
    EditAlreadyRunning,
    FaceForReadingRefused {
        family: String,
        why: Box<Refusal>,
    },
    EditingRestricted,
    BlockNotRead,
    RowNotRead,
    NeedsANewGlyph {
        character: char,
        why: Box<Refusal>,
    },
    AmbiguousCode {
        character: char,
        codes: usize,
    },
    ToUnicodeUnreadable,
    NoToUnicode,
    FontHasTheCodes {
        why: Box<Refusal>,
    },
    PastTheFrame {
        side: Side,
        over: f64,
        why: Box<Refusal>,
    },
    BlockSharesOperators {
        why: Box<Refusal>,
    },
    BlockWillNotMove(BlockMove),
    PictureWillNotMove(PictureMove),
    Stamp(StampWhy),
    EmptyLineNeedsLayout {
        why: Box<Refusal>,
    },
    DeleteNeedsLayout {
        why: Box<Refusal>,
    },
    DeleteAcrossLines {
        why: Box<Refusal>,
    },
    EditAcrossLines {
        why: Box<Refusal>,
    },
    TypedTextLeftTheBlock {
        why: Box<Refusal>,
    },
    OtherBlockWouldShift {
        why: Box<Refusal>,
    },
    ReadsBackOtherwise {
        read: String,
    },
    PageHasNoSize,
    TurnedPageCannotBeLaidOut,
    Layout {
        why: LayoutWhy,
        engine: String,
    },
    Both(Box<Refusal>, Box<Refusal>),
    Engine(String),
}

impl From<String> for Refusal {
    fn from(engine: String) -> Self {
        Self::Engine(engine)
    }
}

impl From<&str> for Refusal {
    fn from(engine: &str) -> Self {
        Self::Engine(engine.to_owned())
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum BlockMove {
    ClipHasNoArea,
    ClipIsCurved,
    ClipIsConcave,
    FontNotEmbedded,
    SeveralStreams,
    InsideForm,
    FirstRunMidLine,
    OutsideTheStream,
    OffsetNotRepresentable,
    WouldTouchOthers,
    NotProvable,
    SharedStream,
    DrawnThroughPattern,
    AlreadyCropped,
    PlacementLeavesClip,
    PlacementFlat,
    SelectionGone,
    Other(String),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StampWhy {
    UnknownToken,
    UnclosedBrace,
    LoneClosingBrace,
    NoText,
    OneLine,
    PageIsTurned,
    WiderThanThePage,
    NoRoom,
    BadMargin,
    BadSize,
    RangeNotNumbers,
    RangePageZero,
    RangePastTheEnd,
    RangeBackwards,
    RangeNamesNothing,
}

impl StampWhy {
    #[must_use]
    pub fn of(reason: &str) -> Option<Self> {
        use pdf_edit::stamp::why;
        Some(match reason {
            why::UNKNOWN_TOKEN => Self::UnknownToken,
            why::UNCLOSED_BRACE => Self::UnclosedBrace,
            why::LONE_CLOSING_BRACE => Self::LoneClosingBrace,
            why::NO_TEXT => Self::NoText,
            why::ONE_LINE => Self::OneLine,
            why::PAGE_IS_TURNED => Self::PageIsTurned,
            why::WIDER_THAN_THE_PAGE => Self::WiderThanThePage,
            why::NO_ROOM => Self::NoRoom,
            why::BAD_MARGIN => Self::BadMargin,
            why::BAD_SIZE => Self::BadSize,
            why::RANGE_NOT_NUMBERS => Self::RangeNotNumbers,
            why::RANGE_PAGE_ZERO => Self::RangePageZero,
            why::RANGE_PAST_THE_END | why::NO_PAGES => Self::RangePastTheEnd,
            why::RANGE_BACKWARDS => Self::RangeBackwards,
            why::RANGE_NAMES_NOTHING => Self::RangeNamesNothing,
            _ => return None,
        })
    }

    fn english(self) -> &'static str {
        match self {
            Self::UnknownToken => {
                "The wording names something unknown: use {page}, {pages}, {file} or {date}"
            }
            Self::UnclosedBrace => "A { in the wording is never closed",
            Self::LoneClosingBrace => "A } in the wording has no { before it",
            Self::NoText => "There is nothing to put on the page",
            Self::OneLine => "A header, footer or watermark is one line",
            Self::PageIsTurned => "A page turned by other than a quarter turn cannot be stamped",
            Self::WiderThanThePage => {
                "This line is wider than the page between its margins: make it smaller or shorter"
            }
            Self::NoRoom => "This page has no room between its margins",
            Self::BadMargin => "A margin is a distance of 0 or more",
            Self::BadSize => "The size must be more than 0",
            Self::RangeNotNumbers => "Write the pages as numbers, like 1-5, 8, 11-",
            Self::RangePageZero => "Pages are counted from 1",
            Self::RangePastTheEnd => "The document has no page of that number",
            Self::RangeBackwards => "A range ends before it starts",
            Self::RangeNamesNothing => "These pages name no page of the document",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum PictureMove {
    LeavesClip,
    ClipNotSeparable,
    HeldClipHasNoArea,
    HeldClipIsCurved,
    HeldClipIsConcave,
    InsideForm,
    DrawnThroughPattern,
    SharedStream,
    MatrixHasNoArea,
    WouldTouchOthers,
    NotProvable,
    PictureGone,
    Other(String),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LayoutWhy {
    RowsOffPitch,
    NoEmbeddedOutline,
    Kerned,
    MixedTransforms,
    UnwritableColour,
    ShearedOrMirrored,
    NoLineHeight,
    PaintsTextOutside,
    NoUnicodeMeaning,
    CodeForSeveralCharacters,
    EveryCharacterDeleted,
    NotInTheFont,
    ControlCharacters,
    InvisibleOrClips,
    FrameHasNoWidth,
}

pub(crate) fn count(n: usize, thing: &str) -> String {
    if n == 1 {
        format!("1 {thing}")
    } else {
        format!("{n} {thing}s")
    }
}

impl Side {
    fn say(self, lang: Lang) -> &'static str {
        match (lang, self) {
            (Lang::English, Self::Left) => "left",
            (Lang::English, Self::Top) => "top",
            (Lang::English, Self::Right) => "right",
            (Lang::English, Self::Bottom) => "bottom",
        }
    }
}

impl Done {
    #[must_use]
    pub fn say(&self, lang: Lang) -> String {
        match lang {
            Lang::English => self.english(),
        }
    }

    #[expect(clippy::too_many_lines, reason = "one sentence per thing an edit does")]
    fn english(&self) -> String {
        let lang = Lang::English;
        match self {
            Self::MovedRun { capability, hidden } => {
                format!("Moved ({capability}){}", hidden.english())
            }
            Self::MovedBlock {
                commands,
                capability,
                hidden,
            } => format!(
                "Moved the block: {} ({capability}){}",
                count(*commands, "command"),
                hidden.english()
            ),
            Self::MovedGroup { pieces } => format!("Moved the group: {}", count(*pieces, "piece")),
            Self::MovedPicture {
                capability,
                hidden: Hidden::Part,
            } => format!(
                "Moved the object under its crop ({capability}) \u{b7} part of it is hidden by a clip this file puts over it"
            ),
            Self::MovedPicture {
                capability,
                hidden: Hidden::All,
            } => format!(
                "Moved the object out of sight ({capability}) \u{b7} a crop this file puts over it hides all of it; its frame is still there to drag it back, and Undo restores it"
            ),
            Self::MovedPicture { capability, .. } => format!("Moved the object ({capability})"),
            Self::ShapedPicture { capability } => format!("Adjusted the object ({capability})"),
            Self::ShapedParagraph { capability } => {
                format!("Adjusted the paragraph ({capability})")
            }
            Self::Sized { points, capability } => {
                format!("Size set to {points:.4} points ({capability})")
            }
            Self::Turned {
                degrees,
                capability,
            } => format!("Turned to {degrees:.4} degrees ({capability})"),
            Self::Slanted {
                degrees,
                capability,
            } => format!("Slanted to {degrees:.4} degrees ({capability})"),
            Self::AnglesUnchanged { capability } => format!("Nothing changed ({capability})"),
            Self::DeletedBlock { glyphs, capability } => {
                format!(
                    "Deleted the block: {} ({capability})",
                    count(*glyphs, "character")
                )
            }
            Self::SetInReadableFace { family } => format!(
                "This block's font draws Lao at the codes of other letters. Read from its \
                 glyphs and set in {family}, so it can be edited; the rest of the document is \
                 untouched"
            ),
            Self::RemovedPicture { capability } => format!("Deleted the object ({capability})"),
            Self::AddedText { characters } => {
                format!("Added text: {}", count(*characters, "character"))
            }
            Self::AddedPicture => "Added a picture".to_owned(),
            Self::AddedPictures { count: n } => format!("Added {}", count(*n, "picture")),
            Self::DrewLine => "Drew a line".to_owned(),
            Self::DrewShape => "Drew a shape".to_owned(),
            Self::FilledField => "Filled in a field".to_owned(),
            Self::AddedField => "Added a form field".to_owned(),
            Self::ChangedField => "Changed a form field's settings".to_owned(),
            Self::MovedFields { count: 1 } | Self::MovedField => "Moved a form field".to_owned(),
            Self::MovedFields { count } => format!("Moved {count} form fields"),
            Self::CopiedFields { count: 1 } => "Pasted a form field".to_owned(),
            Self::CopiedFields { count } => format!("Pasted {count} form fields"),
            Self::ChangedBookmarks => "Changed the bookmarks".to_owned(),
            Self::ChangedLink => "Changed where a link goes".to_owned(),
            Self::ChangedLinkLook => "Changed how a link is drawn".to_owned(),
            Self::NamedAPlace => "Named a place in the document".to_owned(),
            Self::RenamedAPlace => "Renamed a place in the document".to_owned(),
            Self::RemovedAPlace => "Removed a name from the document".to_owned(),
            Self::AddedLink | Self::AddedLinks { count: 1 } => "Added a link".to_owned(),
            Self::AddedLinks { count } => format!("Added {count} links"),
            Self::MovedLinks { count: 1 } | Self::MovedLink => "Moved a link".to_owned(),
            Self::MovedLinks { count } => format!("Moved {count} links"),
            Self::RemovedLinks { count: 1 } | Self::RemovedLink => "Removed a link".to_owned(),
            Self::RemovedLinks { count } => format!("Removed {count} links"),
            Self::OrderedFields => "Changed the order fields are tabbed through".to_owned(),
            Self::RemovedFields { count: 1 } | Self::RemovedField => {
                "Removed a form field".to_owned()
            }
            Self::RemovedFields { count } => format!("Removed {count} form fields"),
            Self::ClearedField => "Cleared a field".to_owned(),
            Self::MarkedThePage => "Marked the page".to_owned(),
            Self::DeletedDrawing => "Deleted a drawing".to_owned(),
            Self::AddedPage { page } => format!("Added a blank page as page {page}"),
            Self::RemovedPages { count } => format!("Deleted {}", count_pages(*count)),
            Self::Described => "Document properties saved".to_owned(),
            Self::RotatedPages { count } => format!("Turned {}", count_pages(*count)),
            Self::Stamped { count } => format!("Stamped {}", count_pages(*count)),
            Self::Recognized {
                pages,
                confidence,
                had_text,
                refused,
            } => {
                let had = if *had_text > 0 {
                    format!("; {} already had text", count_pages(*had_text))
                } else {
                    String::new()
                };
                let unwritten = if *refused > 0 {
                    format!("; {} could not be written", count_pages(*refused))
                } else {
                    String::new()
                };
                format!(
                    "Made {} searchable (the recogniser was {confidence} % sure){had}{unwritten}",
                    count_pages(*pages)
                )
            }
            Self::NothingToRecognize => "Every page named already has text".to_owned(),
            Self::RecognitionStopped => "Stopped; nothing was written".to_owned(),
            Self::RecognitionOutdated => {
                "The document changed while it was being read, so nothing was written; \
                 run it again"
                    .to_owned()
            }
            Self::MovedPages { count, to } => {
                format!("Moved {} to page {to}", count_pages(*count))
            }
            Self::InsertedPages { count, at } => format!(
                "Put in {} from another document, from page {at}",
                count_pages(*count)
            ),
            Self::Undone => "Undone".to_owned(),
            Self::Redone => "Redone".to_owned(),
            Self::NothingToUndo => "Nothing to undo".to_owned(),
            Self::NothingToRedo => "Nothing to redo".to_owned(),
            Self::NothingToWalk => "No step to undo or redo".to_owned(),
            Self::FrameRestored => "Frame size restored".to_owned(),
            Self::NothingChanged => "Nothing changed".to_owned(),
            Self::NothingToDeleteHere => "Nothing to delete here".to_owned(),
            Self::Styled => format!("Text styled{SEP}laid out in its frame"),
            Self::Typed {
                deleted,
                layout,
                overflow,
                brought_in,
                drawn_from,
                cropped,
            } => {
                let done = if *deleted { "Deleted" } else { "Text edited" };
                let mut said = match layout {
                    Layout::InFrame => format!("{done}{SEP}laid out in its frame"),
                    Layout::OnItsRow(why) => format!(
                        "{done}{SEP}this block cannot be laid out again ({}), so the line was not wrapped",
                        why.say(lang)
                    ),
                };
                if *overflow {
                    said.push_str(SEP);
                    said.push_str("a word is wider than the frame and sticks out of its side");
                }
                if let Some(family) = brought_in {
                    said.push_str(SEP);
                    said.push_str("written in ");
                    said.push_str(family);
                    said.push_str(
                        ", at the size and colour of the text it follows, because that text's \
                         own font has no letter for what you typed",
                    );
                }
                if let Some(family) = drawn_from {
                    said.push_str(SEP);
                    said.push_str("this text's font is not in the file, so it is drawn from ");
                    said.push_str(family);
                    said.push_str(", and what you typed is drawn from it too");
                }
                if *cropped {
                    said.push_str(SEP);
                    said.push_str(
                        "part of it is behind a crop this file puts over the page, which hides \
                         it without changing it",
                    );
                }
                said
            }
            Self::DeletedOnRow {
                clusters,
                gap_closed,
                not_laid_out,
            } => {
                let gap = if *gap_closed {
                    "the text after it moved up to close the gap"
                } else {
                    "the gap stays, because closing it would push the next line too"
                };
                format!(
                    "Deleted ({}){SEP}{gap}{SEP}this block cannot be laid out again ({})",
                    count(*clusters, "cluster"),
                    not_laid_out.say(lang)
                )
            }
        }
    }
}

impl Refusal {
    #[must_use]
    pub fn say(&self, lang: Lang) -> String {
        match lang {
            Lang::English => self.english(),
        }
    }

    #[expect(
        clippy::too_many_lines,
        reason = "a table of sentences: its length is how many there are"
    )]
    fn english(&self) -> String {
        let lang = Lang::English;
        match self {
            Self::EditAlreadyRunning => "Another edit is still running".to_owned(),
            Self::FaceForReadingRefused { family, why } => format!(
                "This block's font draws Lao at the codes of other letters, and it could not be \
                 set in {family} to be edited \u{2014} {}",
                why.say(lang)
            ),
            Self::EditingRestricted => "The author of this document restricted editing it. To \
                edit it anyway, choose Edit > Allow editing"
                .to_owned(),
            Self::BlockNotRead => "This block is not on a page that has been read".to_owned(),
            Self::RowNotRead => "This line is not on a page that has been read".to_owned(),
            Self::NeedsANewGlyph { character, why } => format!(
                "This text's font has no \u{201c}{character}\u{201d}, and no font can be added for it here \u{2014} {}",
                why.say(lang)
            ),
            Self::AmbiguousCode { character, codes } => format!(
                "This font has {codes} codes that read as \u{201c}{character}\u{201d}{SEP}which one is not something to guess (G08)"
            ),
            Self::ToUnicodeUnreadable => format!(
                "This font's /ToUnicode table does not read, so which code means which letter is unknown{SEP}moving and deleting still work"
            ),
            Self::NoToUnicode => format!(
                "The file does not say which letters this font's codes stand for (no /ToUnicode), so it cannot be typed over{SEP}moving and deleting still work"
            ),
            Self::FontHasTheCodes { why } => format!(
                "The font has a code for everything typed, but this spot cannot be edited yet \u{2014} {}",
                why.say(lang)
            ),
            Self::PastTheFrame { side, over, why } => format!(
                "This line could not be wrapped, so the text goes {over:.2} pt past the frame's {} edge \u{2014} {}{SEP}dragging the frame wider is a way round it, not the cause",
                side.say(lang),
                why.say(lang)
            ),
            Self::BlockSharesOperators { why } => format!(
                "Cannot move: this block is drawn by the same commands as another block, so it has to be laid out again, and cannot be \u{2014} {}",
                why.say(lang)
            ),
            Self::BlockWillNotMove(why) => why.english(),
            Self::PictureWillNotMove(why) => why.english(),
            Self::Stamp(why) => why.english().to_owned(),
            Self::EmptyLineNeedsLayout { why } => format!(
                "Cannot type on an empty line of this block, because it cannot be laid out again ({})",
                why.say(lang)
            ),
            Self::DeleteNeedsLayout { why } => {
                format!("Cannot delete in this block ({})", why.say(lang))
            }
            Self::DeleteAcrossLines { why } => format!(
                "Cannot delete across lines in this block, because it cannot be laid out again ({})",
                why.say(lang)
            ),
            Self::EditAcrossLines { why } => format!(
                "Cannot edit across lines in this block, because it cannot be laid out again ({})",
                why.say(lang)
            ),
            Self::TypedTextLeftTheBlock { why } => format!(
                "What was typed would not belong to this block, so nothing was changed ({})",
                why.say(lang)
            ),
            Self::OtherBlockWouldShift { why } => format!(
                "Typing on this line would shift letters of another block drawn by the same command, so nothing was changed ({})",
                why.say(lang)
            ),
            Self::ReadsBackOtherwise { read } => format!(
                "Laid out, the text reads back differently from what was typed, so nothing was changed: {read:?}"
            ),
            Self::PageHasNoSize => "This page has no size that can be drawn".to_owned(),
            Self::TurnedPageCannotBeLaidOut => {
                "Text on a turned page cannot be laid out again yet".to_owned()
            }
            Self::Layout { why, engine } => {
                let said = match why {
                    LayoutWhy::RowsOffPitch => {
                        "The lines of this paragraph overlap or are off its line spacing, so which line belongs to which object is uncertain"
                    }
                    LayoutWhy::NoEmbeddedOutline => {
                        "This paragraph's font does not embed its letter outlines in the file"
                    }
                    LayoutWhy::Kerned => "This paragraph adjusts the spacing between letters (TJ)",
                    LayoutWhy::MixedTransforms => {
                        "Parts of this paragraph are placed by different transforms (cannot be laid out together yet)"
                    }
                    LayoutWhy::UnwritableColour => {
                        "This paragraph mixes colours in a colour space that cannot be written back yet"
                    }
                    LayoutWhy::ShearedOrMirrored => {
                        "The text of this paragraph is slanted or mirrored"
                    }
                    LayoutWhy::NoLineHeight => "This paragraph's line spacing is unknown",
                    LayoutWhy::PaintsTextOutside => {
                        "The commands that draw this paragraph also draw other text"
                    }
                    LayoutWhy::NoUnicodeMeaning => {
                        "The file does not say which letters some of this paragraph's text is"
                    }
                    LayoutWhy::CodeForSeveralCharacters => {
                        "A letter code here stands for several letters at once"
                    }
                    LayoutWhy::EveryCharacterDeleted => {
                        "A paragraph cannot be emptied one letter at a time yet \u{b7} select its frame and press Delete to delete the whole paragraph"
                    }
                    LayoutWhy::NotInTheFont => {
                        "This paragraph's font has no code for what was typed"
                    }
                    LayoutWhy::ControlCharacters => {
                        "Tabs and control characters cannot be typed into a paragraph"
                    }
                    LayoutWhy::InvisibleOrClips => {
                        "The text of this paragraph is invisible or used as a clipping path"
                    }
                    LayoutWhy::FrameHasNoWidth => "The frame has no width",
                };
                format!("{said} \u{2014} {engine}")
            }
            Self::Both(first, second) => format!("{}{SEP}{}", first.say(lang), second.say(lang)),
            Self::Engine(said) => said.clone(),
        }
    }
}

impl BlockMove {
    fn english(&self) -> String {
        let said = match self {
            Self::ClipHasNoArea => {
                "Cannot move: the frame clipping this block has no area, so staying inside it cannot be proved"
            }
            Self::ClipIsCurved => {
                "Cannot move: the frame clipping this block has curved edges, so staying inside it cannot be proved"
            }
            Self::ClipIsConcave => {
                "Cannot move: the frame clipping this block is concave, so staying inside it cannot be proved"
            }
            Self::FontNotEmbedded => {
                "Cannot move: the file does not embed this text's font, so where its ink lies is unknown"
            }
            Self::SeveralStreams => {
                "Cannot move: this block is written in more than one content stream"
            }
            Self::InsideForm => {
                "Cannot move: part of this block is drawn through a Form and part is not, so one edit cannot reach both"
            }
            Self::FirstRunMidLine => {
                "Cannot move: this block's first piece of text does not start its line, so the block cannot move as one"
            }
            Self::OutsideTheStream => {
                "Cannot move: part of the text is outside the stream this edit writes"
            }
            Self::OffsetNotRepresentable => {
                "Cannot move: the distance dragged cannot be expressed in this text's coordinates"
            }
            Self::WouldTouchOthers => {
                "Cannot move: this move was proved to affect text not selected"
            }
            Self::NotProvable => "Cannot move: this move cannot be proved safe, so it was not made",
            Self::SharedStream => {
                "Cannot move: this content stream is used in several places, and editing it would change them too"
            }
            Self::DrawnThroughPattern => "Cannot move: this text is drawn through a pattern",
            Self::AlreadyCropped => {
                "Cannot adjust: a clip already crosses this text, so moving it would change what shows"
            }
            Self::PlacementLeavesClip => {
                "Cannot adjust: the new size or angle would take the text outside its clipping frame"
            }
            Self::PlacementFlat => "Cannot adjust: the result would flatten the text to nothing",
            Self::SelectionGone => "Cannot move: the selected text is no longer on this page",
            Self::Other(engine) => return format!("Cannot move: {engine}"),
        };
        said.to_owned()
    }
}

impl PictureMove {
    fn english(&self) -> String {
        let said = match self {
            Self::LeavesClip => {
                "Cannot move: this picture would leave its clipping frame and be cut off"
            }
            Self::ClipNotSeparable => {
                "Cannot move: this picture's clip cannot be safely separated from other content"
            }
            Self::HeldClipHasNoArea => {
                "Cannot move: the clip that has to stay in place has no area, so this cannot be proved"
            }
            Self::HeldClipIsCurved => {
                "Cannot move: the clip that has to stay in place has curved edges, so the picture staying inside it cannot be proved"
            }
            Self::HeldClipIsConcave => {
                "Cannot move: the clip that has to stay in place is concave, so the picture staying inside it cannot be proved"
            }
            Self::InsideForm => {
                "Cannot move: this picture is drawn through a Form whose place in this page's resources cannot be repointed"
            }
            Self::DrawnThroughPattern => "Cannot move: this picture is drawn through a pattern",
            Self::SharedStream => {
                "Cannot move: this content stream is used in several places, and editing it would change them too"
            }
            Self::MatrixHasNoArea => {
                "Cannot move: the matrix placing this picture has no area, so its new place cannot be worked out"
            }
            Self::WouldTouchOthers => {
                "Cannot move: this move was proved to affect things not selected"
            }
            Self::NotProvable => "Cannot move: this move cannot be proved safe, so it was not made",
            Self::PictureGone => "Cannot move: the selected picture is no longer on this page",
            Self::Other(engine) => return format!("Cannot move: {engine}"),
        };
        said.to_owned()
    }
}

impl fmt::Display for Done {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.say(Lang::English))
    }
}

impl fmt::Display for Refusal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.say(Lang::English))
    }
}

fn count_pages(count: usize) -> String {
    if count == 1 {
        "1 page".to_owned()
    } else {
        format!("{count} pages")
    }
}

#[cfg(test)]
mod tests {
    use super::{
        BlockMove, Done, Hidden, Lang, Layout, LayoutWhy, PictureMove, Refusal, Side, StampWhy,
    };

    fn engine() -> Refusal {
        Refusal::Engine("cannot type here".to_owned())
    }

    #[test]
    fn every_edit_answer_is_said_in_every_language_with_its_parts() {
        let done = [
            Done::MovedBlock {
                commands: 7,
                capability: "Exact".to_owned(),
                hidden: Hidden::Nothing,
            },
            Done::Typed {
                deleted: false,
                layout: Layout::OnItsRow(Box::new(engine())),
                overflow: true,
                brought_in: Some("Noto Sans Thai".to_owned()),
                drawn_from: None,
                cropped: true,
            },
            Done::DeletedOnRow {
                clusters: 3,
                gap_closed: false,
                not_laid_out: Box::new(engine()),
            },
            Done::Undone,
            Done::Recognized {
                pages: 12,
                confidence: 86,
                had_text: 3,
                refused: 1,
            },
            Done::NothingToRecognize,
            Done::RecognitionStopped,
            Done::RecognitionOutdated,
        ];
        for answer in &done {
            for lang in Lang::ALL {
                assert!(!answer.say(*lang).trim().is_empty(), "{answer:?}");
            }
        }
        let refusals = [
            Refusal::PastTheFrame {
                side: Side::Right,
                over: 12.5,
                why: Box::new(engine()),
            },
            Refusal::NeedsANewGlyph {
                character: 'X',
                why: Box::new(engine()),
            },
            Refusal::BlockWillNotMove(BlockMove::Other("rule 31".to_owned())),
            Refusal::PictureWillNotMove(PictureMove::LeavesClip),
            Refusal::Layout {
                why: LayoutWhy::Kerned,
                engine: "kerned with TJ".to_owned(),
            },
            Refusal::Stamp(StampWhy::WiderThanThePage),
        ];
        for lang in Lang::ALL {
            let past = refusals[0].say(*lang);
            assert!(past.contains("12.50"), "{past}");
            assert!(refusals[1].say(*lang).contains("cannot type here"));
            assert!(refusals[2].say(*lang).contains("rule 31"));
            assert!(refusals[4].say(*lang).contains("kerned with TJ"));
            assert!(done[0].say(*lang).contains('7'));
        }
        assert!(done[0].say(Lang::English).contains("7 commands"));
        let one = Done::MovedGroup { pieces: 1 };
        assert!(one.say(Lang::English).ends_with("1 piece"), "{one}");
        assert!(refusals[0].say(Lang::English).contains("right"));
    }

    #[test]
    fn every_stamp_refusal_is_worded() {
        for reason in pdf_edit::stamp::why::ALL {
            let why = StampWhy::of(reason).unwrap_or_else(|| panic!("{reason} has no name"));
            let said = Refusal::Stamp(why);
            assert!(!said.say(Lang::English).trim().is_empty(), "{reason}");
        }
        assert_eq!(StampWhy::of("something else"), None);
    }

    #[test]
    fn an_engine_sentence_is_carried_unchanged() {
        let said = Refusal::from("malformed anchor");
        for lang in Lang::ALL {
            assert_eq!(said.say(*lang), "malformed anchor");
        }
        assert_eq!(said.to_string(), "malformed anchor");
    }
}
