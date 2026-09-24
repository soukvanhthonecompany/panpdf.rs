mod controls;
mod edits;
mod facts;
mod home;

pub use controls::Control;
pub use edits::{BlockMove, Done, Hidden, Layout, LayoutWhy, PictureMove, Refusal, Side, StampWhy};
pub use facts::Fact;
pub use home::Home;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Lang {
    #[default]
    English,
}

impl Lang {
    #[must_use]
    pub const fn tag(self) -> &'static str {
        match self {
            Self::English => "en",
        }
    }

    #[must_use]
    pub fn of_tag(tag: &str) -> Option<Self> {
        match tag {
            "en" => Some(Self::English),
            _ => None,
        }
    }

    #[must_use]
    pub const fn endonym(self) -> &'static str {
        match self {
            Self::English => "English",
        }
    }

    pub const ALL: &'static [Self] = &[Self::English];
}

const SEP: &str = " \u{00b7} ";

#[derive(Clone, Debug, PartialEq)]
pub enum Message {
    UnsavedChanges,
    SaveBeforeLeaving,
    DiscardChanges,
    CancelLeaving,
    ResolveDraftBeforeSaving,
    SaveWaitsForTheRunningEdit,
    SaveWaitsForTypingToLand,
    SaveWaitsForTheDraft,
    SaveWaitsForTheDocumentToOpen,
    EditingRestricted,
    EditingRestrictedWarning,
    EditAnyway,
    ReadOnly,
    DocumentIsLocked(String),
    AskForThePassword,
    PasswordRefused,
    EitherPasswordOpensIt,
    Unlock,
    ShowThePassword,
    LeftLocked,
    LinkToPage(usize),
    LinkToMissingPage,
    LinkToAddress(String),
    LinkToUnopenableAddress(String),
    LinkDoesSomethingElse(LinkKind),
    LinkPointsNowhere,

    DraftBlocksDelete,
    DraftTooLongForOneEdit {
        bytes: usize,
        limit: usize,
    },
    NoDraftToRetry,
    DraftWaitsForAnotherEdit,
    DraftPlaceIsGone,
    DraftPlaceIsUncertain,

    DocumentReplacedUnderDraft,
    EditFailedUnexpectedly,
    PageWillNotReadBack(usize),
    CaretLostAfterEdit,
    ClickedAwayFromDraft,
    LeftTheTextUnderDraft,
    CaretIsNoLongerInText,
    TypeHereToAddText,
    NoTextWasTyped,
    ClickToPlacePicture,
    ClickToPlacePictures(usize),
    DropToPlacePictures,
    DropToOpen,
    ThisPageIsAScan,
    PagesChosen(usize),
    DropToInsertPages,
    DragToDrawALine,
    DragToMarkThePage,
    DragOutAShape,
    HitOf {
        at: usize,
        count: usize,
        searching: bool,
    },
    NothingFound {
        searching: bool,
    },
    Close,
    ReplaceAllQuestion {
        count: usize,
        pages: usize,
    },
    ReplaceAllYes,
    ReplacedAll {
        done: usize,
        refused: usize,
    },
    Replacing {
        done: usize,
        count: usize,
    },
    NotReplaced(String),
    ReplacementHoldsTheSearch,
    FillTheShape,
    AnotherEditIsRunning,
    FieldIsReadOnly,
    DragOutAField,
    DragOutALink,
    LinkGoesTo,
    LinkToAPageOfThis,
    LinkToAWebAddress,
    LinkPageNumber,
    LinkAddress,
    PutTheLinkOn,
    TakeTheLinkOff,
    LinkNeedsAPageOfThis,
    LinkNeedsAWebAddress,
    LinkProperties,
    LinkInvisible,
    LinkVisible,
    LinkHighlight,
    LinkHighlightInvert,
    LinkHighlightOutline,
    LinkIsAlreadyThat,
    LinkZoom,
    LinkInheritZoom,
    LinkFitPage,
    LinkFitWidth,
    LinkFitHeight,
    LinkFitVisible,
    LinkActualSize,
    LinkZoomTo,
    LinkNeedsAZoom,
    LinkTheAddressesWritten,
    LinkTheAddressesWrittenWhy,
    NoAddressesToLink,
    LinksChosen(usize),
    LinkToAnotherDocument,
    LinkFileHint,
    LinkNeedsAFile,
    LinkNeedsAPageNumber,
    LinkToANamedPlace,
    ChooseANamedPlace,
    NoNamedPlacesYet,
    LinkNeedsAName,
    NamedPlaces,
    NamedPlacesWhy,
    StampWhy,
    StampInsert,
    StampTokenPage,
    StampTokenPages,
    StampTokenFile,
    StampWhere,
    StampHeader,
    StampFooter,
    StampMiddle,
    StampColour,
    StampOpacity,
    StampMargin,
    StampPages,
    StampPagesHint,
    StampOnly(pdf_edit::stamp::Only),
    StampStart,
    StampPreview {
        page: usize,
        line: String,
    },
    StampPreviewNotShown {
        page: usize,
    },
    StampApply(usize),
    OcrWhy,
    OcrLanguages,
    OcrLanguage(String),
    OcrSkipText,
    OcrStart(usize),
    OcrStop,
    OcrProgress {
        done: usize,
        total: usize,
    },
    OcrNotInstalled,
    OcrNoLanguage,
    OcrModel,
    OcrQuality(pdf_ocr::Quality),
    OcrErrorRate {
        code: String,
        cer: f32,
    },
    OcrGetModel {
        code: String,
        bytes: u64,
    },
    OcrGettingModel(String),
    OcrModelFailed(String),
    OcrGetEngine,
    OcrGettingEngine,
    OcrEngineFailed(String),
    OcrEngineElsewhere,
    OcrThisPage,
    PrintWhy,
    AllPages,
    PrintCurrentPage,
    SomePages,
    PrintReverse,
    PrintPaper,
    PrintOrientation,
    PrintOrientationIs(pdf_print::Orientation),
    PrintPerSheet,
    PrintCustomGrid,
    PrintScaling(PrintScalingKind),
    PrintOrder,
    PrintOrderIs(pdf_print::Order),
    PrintBorders,
    PrintAutoRotate,
    PrintFitAgain,
    PrintMargin,
    PrintMarginOfPrinter {
        millimetres: String,
    },
    PrintSheet {
        at: usize,
        of: usize,
    },
    PrintSummary {
        pages: usize,
        sheets: usize,
        paper: String,
    },
    PrintButton,
    PrintPrinter,
    PrintNoPrinter,
    PrintNoService(String),
    PrintCopies,
    PrintColour,
    PrintMonochrome,
    PrintSides,
    PrintSidesIs(pdf_print::service::Sides),
    PrintPaperMissing {
        printer: String,
        paper: String,
    },
    PrintEdgeTooNarrow {
        printer: String,
        millimetres: String,
    },
    PrintPreparing {
        done: usize,
        total: usize,
    },
    PrintSent {
        sheets: usize,
        printer: String,
        job: Option<i32>,
    },
    PrintFailed(String),
    PrintRefused,
    PrintDegraded,
    PrintDrawing,
    PrintLayout(pdf_print::LayoutError),
    PrintSheetFailed(String),

    SplitWhy,
    SplitEvery,
    SplitEveryPages,
    SplitAtPages,
    SplitInto {
        pages: usize,
        files: usize,
    },
    SplitWhere,
    TakingPagesOut,
    WroteFiles {
        files: usize,
        first: String,
        bytes: u64,
    },
    CouldNotTakePagesOut(String),

    ExportWhy,
    ExportResolution,
    ExportDpi,
    ExportSize {
        pages: usize,
        width: u32,
        height: u32,
    },
    DrawingPictures,
    WrotePictures {
        files: usize,
        first: String,
        bytes: u64,
    },
    CouldNotWritePictures(String),
    MakingPages,
    CouldNotMakePages(String),
    NameThisPage,
    NameHint,
    NameIsNotOne,
    RenameThePlace,
    RemoveThePlace,
    RemoveThePlaceWhy,
    FieldText,
    FieldParagraph,
    FieldCheckbox,
    FieldRadio,
    FieldDropdown,
    FieldChoices,
    FieldGroup,
    FieldNewGroup,
    Contents,
    AddBookmarkSaid,
    RenameBookmark,
    BookmarkUp,
    BookmarkDown,
    BookmarkIn,
    BookmarkOut,
    BookmarkName,
    FieldsPanel,
    NoFieldsYet,
    OrderByRow,
    OrderByColumn,
    OrderByRowSaid,
    OrderByColumnSaid,
    FieldPointer,
    ArrangeFields,
    AlignLeftEdges,
    AlignRightEdges,
    AlignTops,
    AlignBottoms,
    AlignCentresAcross,
    AlignCentresDown,
    DistributeAcross,
    DistributeDown,
    MatchWidth,
    MatchHeight,
    MatchSize,
    CentreAcrossPage,
    CentreDownPage,
    FieldListBox,
    FieldNone,
    FieldDate,
    FieldSignature,
    FieldButton,
    FieldCaption,
    FieldLink,
    FieldDateFormat,
    FieldNotADate,
    FieldNoLink,
    FieldProperties,
    FieldGeneral,
    FieldAppearance,
    FieldOptions,
    FieldTooltip,
    FieldVisibility,
    FieldVisible,
    FieldHidden,
    FieldVisibleNotPrinted,
    FieldHiddenPrinted,
    FieldBorderColour,
    FieldFillColour,
    FieldLineThickness,
    FieldThin,
    FieldMedium,
    FieldThick,
    FieldLineStyle,
    FieldSolid,
    FieldDashed,
    FieldBeveled,
    FieldInset,
    FieldDefaultValue,
    FieldMultiline,
    FieldPassword,
    FieldScroll,
    FieldSpellCheck,
    FieldLimit,
    FieldComb,
    FieldMarkStyle,
    MarkCheck,
    MarkCircle,
    MarkCross,
    MarkDiamond,
    MarkSquare,
    MarkStar,
    FieldExportValue,
    FieldCheckedByDefault,
    FieldInUnison,
    FieldItem,
    FieldAddItem,
    FieldDeleteItem,
    FieldItemUp,
    FieldItemDown,
    FieldSort,
    FieldCustomText,
    FieldMultiSelect,
    FieldCommitAtOnce,
    FieldNoOptions,
    FieldSettings,
    FieldName,
    FieldRequired,
    FieldReadOnly,
    FieldAlignment,
    AlignLeft,
    AlignCentre,
    AlignRight,
    FieldTextSize,
    FieldAutoSize,
    Apply,
    DeleteField,
    SignatureFieldsAreNotSignedYet,
    HowToFinishAField,

    Command(Command),
    PageOf {
        page: usize,
        count: usize,
    },
    ZoomPercent(u32),

    Opening(String),
    OpeningFailed,
    StartedANewDocument,
    DocumentCount(usize),
    ObjectCount(usize),
    SavedTo {
        name: String,
        bytes: u64,
    },
    PagesNotRead {
        name: String,
        why: String,
    },
    PictureNotRead {
        name: String,
        why: String,
    },
    CouldNotSave {
        name: String,
        why: String,
    },
    DraftCopied,
    DraftDiscarded,
    PageWillNotOpen {
        page: usize,
        why: String,
    },

    FrameDeclared {
        wide: f64,
        high: f64,
        relaid: bool,
    },
    FrameKeptNotRelaid {
        wide: f64,
        high: f64,
        why: String,
    },
    ParagraphOf {
        rows: usize,
        runs: usize,
    },
    ObjectSized {
        kind: ObjectKind,
        wide: f64,
        high: f64,
    },
    Selected(usize),
    ObjectKind(ObjectKind),

    WentToPage(usize),
    LinkLeadsOffTheDocument,
    WillNotOpenFromDocument(String),
    CouldNotOpen {
        uri: String,
        why: String,
    },
    Opened(String),
    LinkKindNotOpenable,

    PageSaid {
        page: usize,
        said: String,
    },
    AndPressesRefused {
        said: Box<Message>,
        refused: u64,
    },
    DoubleClickToTypeHere,
    DraftIsCollectingWhatYouType,
    RangeCannotBeCopied,
    Copied(usize),

    SelectionOf {
        shown: String,
        clusters: usize,
        unreadable: usize,
    },
    SelectionOfUnread(usize),
    PressDeleteToRemove,
    DeleteBlockHelp,
    DeletePlannableHere,
    DeleteNotPlannableHere,
    EditingText,
    EditingTextHelp,
    DragToSelect,
    BlockOfRuns {
        runs: usize,
        lines: usize,
    },
    BlockOfRunsHelp {
        runs: usize,
        lines: usize,
    },
    Size,
    SizeOfSelectionHelp,
    SizeOfBlockHelp,
    LineSpacing,
    LineSpacingHelp,
    AlignStartHelp,
    AlignCentreHelp,
    AlignEndHelp,
    AlignJustifyHelp,
    FlowRoundHelp,
    FlowsRoundNow,
    NothingInTheWay,
    FlowRoundFellBehind,
    TextFlowsRoundThis,
    OrderingWaitsForThePage,
    TextMadeWay {
        blocks: usize,
    },
    Times,
    TimesTextSize {
        ratio: f64,
        points: f64,
    },
    Bold,
    BoldHelp,
    Italic,
    ItalicHelp,
    Underline,
    NoUnderline,
    PlainFace,
    PlainFaceHelp,
    Font,
    FontHelp,
    GrowFont,
    ShrinkFont,
    ClearFormatting,
    TextColour,
    StyleForNextTyping,
    NothingToFormat,
    PaintSelection(String),
    Turn,
    TurnHelp,
    Slant,
    SlantHelp,
    CaretGoneBeforeStyling,
    CaretGoneBeforeTyping,
    CaretGoneBeforeDeleting,
    BlockTextNotFound,
    GroupLetGoAfterTheEdit,

    DraftBarTitle,
    DraftRetry,
    DraftCopy,
    DraftDiscard,
    DraftWaitForTheEditToLand,
    DraftPlaceNoLongerUsable,

    AiTitle,
    AiBaseUrl,
    AiApiKey,
    AiModel,
    AiProvider,
    AiAttach,
    AiHistory,
    AiNewChatTitle,
    AiNoChatsYet,
    AiUntitledChat,
    AiForgetChat,
    AiKeepTheKey,
    AiKeepTheKeyMeans,
    AiKeptKeyUnreadable,
    AiNowOnDocument(String),
    AiResetToDefault,
    AiFullAccessAsk,
    AiFullAccessMeans,
    AiFullAccessConfirm,
    AiAdd,
    AiAttachFiles,
    AiAttachFilesMeans,
    AiEffortOffMeans,
    AiEffortNoneMeans,
    AiEffortLowMeans,
    AiEffortMediumMeans,
    AiEffortHighMeans,
    AiTestConnection,
    AiCancel,
    AiDisconnect,
    AiConnected,
    AiConnectedTo,
    AiNotConnected,
    AiCheckingConnection,
    AiNotConnectedYet,
    AiThisDocumentsChat,
    AiEdit,
    AiEditMeans,
    AiAskAgain,
    AiAskAgainMeans,
    AiWentBack,
    AiWentBackDocumentStays,
    AiPutBack,
    AiChatMenu,
    AiCopyWholeChat,
    AiDeleteThisChatSure,
    AiThisDocument,
    AiOtherChats,
    AiQuestionForYou,
    AiOwnAnswer,
    AiAnswer,
    AiSkipQuestion,
    AiWritingTheAnswer,
    AiWaitingForModel(String),
    AiWritingPieces {
        written: usize,
        pieces: usize,
    },
    AiAskHint,
    AiIncludeContext {
        characters: usize,
    },
    AiSend,
    AiResponse,
    AiCopyResponse,
    AiPrivacy,
    AiWorkerStopped,
    AiNothingAskedYet,
    AiNewChat,
    AiConnection,
    AiYou,
    AiThinking,
    AiKeyNeeded,
    AiFindModels,
    AiNoModelChosen,
    AiEnterSends,
    AiSendThePage,
    AiConnectionInvalid,
    AiStopped,
    AiCouldNotReach,
    AiAnswerTooLarge,
    AiServiceRefused,
    AiAnswerUnexpected,
    AiDismiss,
    AiWantsTo,
    AiAllowOnce,
    AiAllowForThisChat,
    AiRefuse,
    AiChangedTheDocument {
        page: usize,
    },
    AiTooManyRounds,
    AiEffort,
    AiEffortOff,
    AiEffortNone,
    AiEffortLow,
    AiEffortMedium,
    AiEffortHigh,
    AiMode,
    AiModeChatOnly,
    AiModeAskBeforeChanges,
    AiModeDoIt,
    AiModeFree,
    AiModeChatOnlyWhat,
    AiModeAskBeforeChangesWhat,
    AiModeDoItWhat,
    AiModeFreeWhat,

    AgentsTitle,
    AgentsWhat,
    AgentsForClaudeCode,
    AgentsForClients,
    AgentsNote,
    AgentsCopied,

    Done(Done),
    Refused(Refusal),
    Home(Home),
    Control(Control),
    Quiet,
    DrawingSpeed(crate::speed::Summary),
    EditSpeed(pdf_session::stages::Stages),

    Plain(String),

    NoGraphics(String),

    DropToAttach,
    AiAttachPicture,
    AiAttachPdf,
    AiAttachText,
    AiAttachRefused,
    AiThinkingAloud,
    AiCopyCode,
    AiAPicture,
    AiAnswerCutShort,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Command {
    Open,
    Save,
    Undo,
    Redo,
    Copy,
    Cut,
    Paste,
    PasteInPlace,
    Select,
    Text,
    Find,
    FindNext,
    FindPrevious,
    Replace,
    ReplaceWith,
    ReplaceThisOne,
    ReplaceAll,
    Pen,
    Highlighter,
    Shape,
    Form,
    Link,
    Contents,
    Picture,
    Delete,
    BringToFront,
    BringForward,
    SendBackward,
    SendToBack,
    PreviousPage,
    NextPage,
    ZoomIn,
    ZoomOut,
    NewDocument,
    AllowEditing,
    Theme,
    File,
    Edit,
    View,
    Documents,
    ShowFrames,
    Frames,
    FramesOfText,
    FramesOfPictures,
    FramesOfDrawings,
    DarkMode,
    ShowDrawingSpeed,
    Language,
    Help,
    ReportAProblem,
    ShowTheLog,
    Pages,
    HidePages,
    BackToReading,
    Home,
    Page,
    BlankPageBefore,
    BlankPageAfter,
    SameSizeAsThisPage,
    Landscape,
    DeletePage,
    MovePageEarlier,
    MovePageLater,
    MovePageFirst,
    MovePageLast,
    RotateClockwise,
    RotateCounterClockwise,
    StampPageNumbers,
    StampHeaderFooter,
    StampWatermark,
    RecognizeText,
    AiAssistant,
    ConnectAgents,
    Print,
    PagesFromFileBefore,
    PagesFromFileAfter,
    SaveAs,
    DuplicatePage,
    PagesToNewFile,
    SplitDocument,
    PagesAsPictures,
    PdfFromPictures,
    PicturesAsPagesBefore,
    PicturesAsPagesAfter,
    Insert,
    Tools,
    InsertPictures,
    InsertText,
    InsertField,
    InsertBlankPage,
    InsertPagesFromPdf,
    InsertPicturesAsPages,
    InsertHere,
    AddPages,
    MoreForPage,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PrintScalingKind {
    Fit,
    ShrinkOversized,
    ActualSize,
    Custom,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ObjectKind {
    Text,
    Drawing,
    Picture,
    Shading,
    Group,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LinkKind {
    AnotherFile,
    AttachedFile,
    LaunchProgram,
    ReaderCommand,
    SomeAction,
}

impl ObjectKind {
    const fn english(self) -> &'static str {
        match self {
            Self::Text => "Text",
            Self::Drawing => "Drawing",
            Self::Picture => "Picture",
            Self::Shading => "Shading",
            Self::Group => "Group",
        }
    }
}

fn megabytes(bytes: u64) -> String {
    let tenths = bytes.saturating_mul(10).saturating_add(524_288) / 1_048_576;
    format!("{}.{} MB", tenths / 10, tenths % 10)
}

impl Message {
    #[must_use]
    pub fn refusal(error: &pdf_edit::spike_move_text::SpikeError) -> Self {
        match error {
            pdf_edit::spike_move_text::SpikeError::RetypeUnsupported(reason) => {
                StampWhy::of(reason).map_or_else(
                    || Self::Refused(error.to_string().into()),
                    |why| Self::Refused(Refusal::Stamp(why)),
                )
            }
            _ => Self::Refused(error.to_string().into()),
        }
    }

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
        match self {
            Self::UnsavedChanges => "Unsaved changes".to_owned(),
            Self::SaveBeforeLeaving => "Save your changes before leaving this document?".to_owned(),
            Self::DiscardChanges => "Discard changes".to_owned(),
            Self::CancelLeaving => "Cancel".to_owned(),
            Self::SaveWaitsForTheRunningEdit => "An edit is still running".to_owned(),
            Self::SaveWaitsForTypingToLand => {
                "What was typed has not reached the document yet".to_owned()
            }
            Self::SaveWaitsForTheDraft => {
                "A draft has to be retried, copied or discarded first".to_owned()
            }
            Self::SaveWaitsForTheDocumentToOpen => "Another document is still opening".to_owned(),
            Self::EditingRestricted => "Editing is restricted".to_owned(),
            Self::EditingRestrictedWarning => "The author of this document set it so that its \
                content should not be changed. You can edit it anyway: the saved copy keeps the \
                same password protection and permissions. Edit it only if you have the right to."
                .to_owned(),
            Self::EditAnyway => "Edit anyway".to_owned(),
            Self::ReadOnly => "Only read it".to_owned(),
            Self::DocumentIsLocked(name) => format!("{name} is locked"),
            Self::AskForThePassword => {
                "Enter the password that opens this document.".to_owned()
            }
            Self::PasswordRefused => "That is not the password. Try again.".to_owned(),
            Self::EitherPasswordOpensIt => "A document can carry two passwords: one to open it \
                and one to change what it allows. Either one opens it here."
                .to_owned(),
            Self::Unlock => "Unlock".to_owned(),
            Self::ShowThePassword => "Show the password".to_owned(),
            Self::LeftLocked => "The document was not opened: it is locked.".to_owned(),
            Self::ResolveDraftBeforeSaving => "Finish or discard pending text before saving.".to_owned(),
            Self::LinkToPage(page) => format!("Page {page}{SEP}Ctrl+click to follow"),
            Self::LinkToMissingPage => "Link to a page this document does not have".to_owned(),
            Self::LinkToAddress(uri) => format!("{uri}{SEP}Ctrl+click to open"),
            Self::LinkToUnopenableAddress(uri) => {
                format!("{uri}{SEP}this kind of address cannot be opened from here")
            }
            Self::LinkDoesSomethingElse(kind) => {
                let kind = match kind {
                    LinkKind::AnotherFile => "another file",
                    LinkKind::AttachedFile => "an attached file",
                    LinkKind::LaunchProgram => "a program",
                    LinkKind::ReaderCommand => "a reader command",
                    LinkKind::SomeAction => "an action",
                };
                format!("Link to {kind}{SEP}cannot be opened from here")
            }
            Self::LinkPointsNowhere => "Link that points nowhere".to_owned(),

            Self::DraftBlocksDelete => {
                format!("A draft is waiting{SEP}retry, copy or discard it, then delete")
            }
            Self::DraftTooLongForOneEdit { bytes, limit } => {
                format!("{bytes} bytes is past the {limit} one edit takes{SEP}all of it is kept")
            }
            Self::NoDraftToRetry => "No draft to retry".to_owned(),
            Self::DraftWaitsForAnotherEdit => {
                format!("Another edit is still running{SEP}retry once it lands")
            }
            Self::DraftPlaceIsGone => {
                format!(
                    "The place this draft was meant for is gone{SEP}copy it and paste it yourself"
                )
            }
            Self::DraftPlaceIsUncertain => format!(
                "The document changed after this was typed, so its place is no longer certain{SEP}copy it and paste it yourself"
            ),
            Self::DocumentReplacedUnderDraft => format!(
                "Another file was opened before this text reached the document{SEP}copy it and paste it yourself"
            ),
            Self::EditFailedUnexpectedly => {
                "The edit failed unexpectedly \u{2014} the document may not be what you see"
                    .to_owned()
            }
            Self::PageWillNotReadBack(page) => format!(
                "Page {page} will not read back, so the caret cannot be put there{SEP}copy it and paste it yourself"
            ),
            Self::CaretLostAfterEdit => {
                format!(
                    "The caret's place could not be found after the edit{SEP}copy it and paste it yourself"
                )
            }
            Self::ClickedAwayFromDraft => format!(
                "You clicked elsewhere before what you typed reached the document{SEP}copy it and paste it yourself"
            ),
            Self::LeftTheTextUnderDraft => format!(
                "You left the text before this reached the document{SEP}copy it and paste it yourself"
            ),
            Self::CaretIsNoLongerInText => {
                format!("The caret is no longer in text{SEP}copy it and paste it yourself")
            }
            Self::TypeHereToAddText => "Type to put text in the frame you drew".to_owned(),
            Self::ClickToPlacePicture => {
                "Click on a page to place the picture, or drag to give it a size".to_owned()
            }
            Self::ClickToPlacePictures(count) => format!(
                "Click on a page to place the {count} pictures side by side, or drag a box \
                 to lay them out in"
            ),
            Self::DropToPlacePictures => "Let go to put the pictures on this page".to_owned(),
            Self::DropToOpen => "Let go to open the document".to_owned(),
            Self::PagesChosen(count) => format!("{count} chosen"),
            Self::DropToInsertPages => "Let go here to put them in as pages".to_owned(),
            Self::ThisPageIsAScan => {
                "This page is a scan: its words cannot be searched or copied until they are read"
                    .to_owned()
            }
            Self::DragToDrawALine => {
                "Draw on the page; hold shift for a straight line".to_owned()
            }
            Self::DragToMarkThePage => "Drag a box over what you want to highlight".to_owned(),
            Self::DragOutAShape => {
                "Drag out the shape; hold shift to keep it square".to_owned()
            }
            Self::HitOf {
                at,
                count,
                searching,
            } => {
                let more = if *searching { "…" } else { "" };
                format!("{at} of {count}{more}")
            }
            Self::NothingFound { searching } => if *searching {
                "Looking…"
            } else {
                "Not found"
            }
            .to_owned(),
            Self::Close => "Close".to_owned(),
            Self::ReplaceAllQuestion { count, pages } => format!(
                "Replace {} on {}?",
                edits::count(*count, "place"),
                edits::count(*pages, "page")
            ),
            Self::ReplaceAllYes => "Replace them all".to_owned(),
            Self::ReplacedAll { done, refused } if *refused == 0 => {
                format!("Replaced {}", edits::count(*done, "place"))
            }
            Self::ReplacedAll { done, refused } => format!(
                "Replaced {}; {} could not be",
                edits::count(*done, "place"),
                edits::count(*refused, "place")
            ),
            Self::Replacing { done, count } => format!("Replacing {done} of {count}…"),
            Self::NotReplaced(why) => format!("This one could not be replaced: {why}"),
            Self::ReplacementHoldsTheSearch => {
                "The replacement holds what is being searched for, so replacing all of them \
                 would never end. Replace them one at a time."
                    .to_owned()
            }
            Self::FillTheShape => "Fill".to_owned(),
            Self::NoTextWasTyped => "Nothing was typed, so nothing was added".to_owned(),
            Self::AnotherEditIsRunning => "Another edit is already running".to_owned(),
            Self::FieldIsReadOnly => "This field can be read but not changed".to_owned(),
            Self::DragOutAField => "Click fields to choose them (Shift or Ctrl adds), drag to move, arrows nudge, Ctrl+C / Ctrl+V copy. Pick a kind above to add a field".to_owned(),
            Self::DragOutALink => "Drag out a box to make a link, or click one to change where it goes. Delete takes it off".to_owned(),
            Self::LinkGoesTo => "Goes to".to_owned(),
            Self::LinkToAPageOfThis => "A page of this document".to_owned(),
            Self::LinkToAWebAddress => "A web address".to_owned(),
            Self::LinkPageNumber => "Page".to_owned(),
            Self::LinkAddress => "Address".to_owned(),
            Self::PutTheLinkOn => "Add link".to_owned(),
            Self::TakeTheLinkOff => "Remove link".to_owned(),
            Self::LinkNeedsAPageOfThis => "This document has no such page".to_owned(),
            Self::LinkNeedsAWebAddress => {
                "A link opens the web and mail only, as https://example.org or mailto:someone@example.org".to_owned()
            }
            Self::LinkProperties => "Link".to_owned(),
            Self::LinkInvisible => "Invisible rectangle".to_owned(),
            Self::LinkVisible => "Visible rectangle".to_owned(),
            Self::LinkHighlight => "Highlight".to_owned(),
            Self::LinkHighlightInvert => "Invert".to_owned(),
            Self::LinkHighlightOutline => "Outline".to_owned(),
            Self::LinkIsAlreadyThat => "The link is already that".to_owned(),
            Self::LinkZoom => "Zoom".to_owned(),
            Self::LinkInheritZoom => "Inherit zoom".to_owned(),
            Self::LinkFitPage => "Fit page".to_owned(),
            Self::LinkFitWidth => "Fit width".to_owned(),
            Self::LinkFitHeight => "Fit height".to_owned(),
            Self::LinkFitVisible => "Fit visible".to_owned(),
            Self::LinkActualSize | Self::PrintScaling(PrintScalingKind::ActualSize) => {
                "Actual size".to_owned()
            }
            Self::LinkZoomTo => "Zoom to".to_owned(),
            Self::LinkNeedsAZoom => {
                "A zoom is a number between one and six thousand four hundred per cent".to_owned()
            }
            Self::LinkTheAddressesWritten => "Link the addresses written".to_owned(),
            Self::LinkTheAddressesWrittenWhy => {
                "Puts a link over every web address written on this page".to_owned()
            }
            Self::NoAddressesToLink => {
                "No web addresses on this page that are not linked already".to_owned()
            }
            Self::LinksChosen(1) => "1 link chosen".to_owned(),
            Self::LinksChosen(count) => format!("{count} links chosen"),
            Self::LinkToANamedPlace => "A place in this document, by name".to_owned(),
            Self::ChooseANamedPlace => "Choose a named place".to_owned(),
            Self::NoNamedPlacesYet => "This document names no place yet".to_owned(),
            Self::LinkNeedsAName => "Choose the named place to go to".to_owned(),
            Self::NamedPlaces => "Named places".to_owned(),
            Self::NamedPlacesWhy => {
                "A name stands for a place in this document. Links sent to the name follow it \
                 when the place moves."
                    .to_owned()
            }
            Self::NameThisPage => "Name this page".to_owned(),
            Self::NameHint => "chapter two".to_owned(),
            Self::NameIsNotOne => "Type a name for the place".to_owned(),
            Self::StampWhy => {
                "One line on every page chosen. Each page gets its own number; adding it is one \
                 undo."
                    .to_owned()
            }
            Self::StampInsert => "Insert:".to_owned(),
            Self::StampTokenPage => "Page number".to_owned(),
            Self::StampTokenPages => "Page count".to_owned(),
            Self::StampTokenFile => "File name".to_owned(),
            Self::StampWhere => "Where".to_owned(),
            Self::StampHeader => "Header".to_owned(),
            Self::StampFooter => "Footer".to_owned(),
            Self::StampMiddle => "Middle of the page (watermark)".to_owned(),
            Self::StampColour | Self::PrintColour => "Colour".to_owned(),
            Self::StampOpacity => "Opacity".to_owned(),
            Self::StampMargin => "Margin (pt)".to_owned(),
            Self::StampPages => "Pages".to_owned(),
            Self::StampPagesHint => "all, or 1-5, 8, 11-".to_owned(),
            Self::StampOnly(pdf_edit::stamp::Only::Every) => "Every page".to_owned(),
            Self::StampOnly(pdf_edit::stamp::Only::Odd) => "Odd pages only".to_owned(),
            Self::StampOnly(pdf_edit::stamp::Only::Even) => "Even pages only".to_owned(),
            Self::StampStart => "First page of the range is number".to_owned(),
            Self::StampPreview { page, line } => format!("Page {}: \u{201c}{line}\u{201d}", page + 1),
            Self::StampPreviewNotShown { page } => {
                format!("Go to page {} to see it", page + 1)
            }
            Self::StampApply(1) => "Add to 1 page".to_owned(),
            Self::StampApply(count) => format!("Add to {count} pages"),
            Self::OcrWhy => "Reads the words in scanned pages so they can be searched and \
                             copied. It runs on this computer; nothing is sent anywhere."
                .to_owned(),
            Self::OcrLanguages => "Languages on the pages (each one more takes longer)".to_owned(),
            Self::OcrLanguage(code) => match code.as_str() {
                "lao" => "Lao (\u{0ea5}\u{0eb2}\u{0ea7})".to_owned(),
                "tha" => "Thai (\u{0e44}\u{0e17}\u{0e22})".to_owned(),
                "eng" => "English".to_owned(),
                other => other.to_owned(),
            },
            Self::OcrSkipText => "Skip pages that already have text".to_owned(),
            Self::OcrStart(1) => "Read 1 page".to_owned(),
            Self::OcrStart(count) => format!("Read {count} pages"),
            Self::OcrStop => "Stop".to_owned(),
            Self::OcrProgress { done, total } => format!("Read {done} of {total} pages\u{2026}"),
            Self::OcrNotInstalled => "The text recogniser (Tesseract) is not installed. On Linux, \
                                      install tesseract-ocr with tesseract-ocr-lao, \
                                      tesseract-ocr-tha and tesseract-ocr-eng."
                .to_owned(),
            Self::OcrNoLanguage => "Choose at least one language".to_owned(),
            Self::OcrModel => "Models".to_owned(),
            Self::OcrQuality(pdf_ocr::Quality::Accurate) => "Accurate".to_owned(),
            Self::OcrQuality(pdf_ocr::Quality::Fast) => "Fast".to_owned(),
            Self::OcrErrorRate { code, cer } => format!(
                "{}: {cer:.1} % of letters read wrong",
                Self::OcrLanguage(code.clone()).say(Lang::English)
            ),
            Self::OcrGetModel { code, bytes } => format!(
                "Download {} ({})",
                Self::OcrLanguage(code.clone()).say(Lang::English),
                megabytes(*bytes)
            ),
            Self::OcrGettingModel(code) => format!(
                "Getting {}\u{2026}",
                Self::OcrLanguage(code.clone()).say(Lang::English)
            ),
            Self::OcrModelFailed(said) => {
                format!("The model could not be downloaded: {said}")
            }
            Self::OcrGetEngine => "Install the recogniser".to_owned(),
            Self::OcrGettingEngine => "Installing the recogniser\u{2026}".to_owned(),
            Self::OcrEngineFailed(said) => {
                format!("The recogniser could not be installed: {said}")
            }
            Self::OcrEngineElsewhere => "The text recogniser (Tesseract) is not on this \
                                        computer, and this program cannot put it there by \
                                        itself. On Windows, install it from \
                                        https://github.com/UB-Mannheim/tesseract/wiki ; on \
                                        macOS, run: brew install tesseract"
                .to_owned(),
            Self::OcrThisPage => "This page".to_owned(),
            Self::PrintWhy => "Choose the pages, the paper and how they go on it. The preview \
                               shows each sheet as it will print."
                .to_owned(),
            Self::AllPages => "All pages".to_owned(),
            Self::PrintCurrentPage => "Current page".to_owned(),
            Self::SomePages => "Some pages".to_owned(),
            Self::PrintReverse => "Reverse order".to_owned(),
            Self::PrintPaper => "Paper".to_owned(),
            Self::PrintOrientation => "Orientation".to_owned(),
            Self::PrintOrientationIs(pdf_print::Orientation::Auto) => {
                "Auto portrait/landscape".to_owned()
            }
            Self::PrintOrientationIs(pdf_print::Orientation::Portrait) => "Portrait".to_owned(),
            Self::PrintOrientationIs(pdf_print::Orientation::Landscape) => "Landscape".to_owned(),
            Self::PrintPerSheet => "Pages per sheet".to_owned(),
            Self::PrintCustomGrid => "Custom".to_owned(),
            Self::PrintScaling(PrintScalingKind::Fit) => "Fit to paper".to_owned(),
            Self::PrintScaling(PrintScalingKind::ShrinkOversized) => {
                "Shrink oversized pages".to_owned()
            }
            Self::PrintScaling(PrintScalingKind::Custom) => "Custom scale".to_owned(),
            Self::PrintOrder => "Page order".to_owned(),
            Self::PrintOrderIs(pdf_print::Order::Horizontal) => {
                "Across, left to right".to_owned()
            }
            Self::PrintOrderIs(pdf_print::Order::HorizontalReversed) => {
                "Across, right to left".to_owned()
            }
            Self::PrintOrderIs(pdf_print::Order::Vertical) => "Down, then left to right".to_owned(),
            Self::PrintOrderIs(pdf_print::Order::VerticalReversed) => {
                "Down, then right to left".to_owned()
            }
            Self::PrintBorders => "Print page borders".to_owned(),
            Self::PrintAutoRotate => "Turn pages to fit the paper".to_owned(),
            Self::PrintFitAgain => {
                "Put the page back in the middle, at the size that fits".to_owned()
            }
            Self::PrintMargin => "Margins".to_owned(),
            Self::PrintMarginOfPrinter { millimetres } => {
                format!("Printer's margin: {millimetres} mm")
            }
            Self::PrintSheet { at, of } => format!("Sheet {at} of {of}"),
            Self::PrintSummary {
                pages: 1,
                sheets: 1,
                paper,
            } => format!("1 page on 1 sheet of {paper}"),
            Self::PrintSummary {
                pages,
                sheets: 1,
                paper,
            } => format!("{pages} pages on 1 sheet of {paper}"),
            Self::PrintSummary {
                pages,
                sheets,
                paper,
            } => format!("{pages} pages on {sheets} sheets of {paper}"),
            Self::PrintButton => "Print".to_owned(),
            Self::PrintPrinter => "Printer".to_owned(),
            Self::PrintNoPrinter => "No printer is set up on this computer.".to_owned(),
            Self::PrintNoService(why) => format!("The print service can't be reached: {why}"),
            Self::PrintCopies => "Copies".to_owned(),
            Self::PrintMonochrome => "Black and white".to_owned(),
            Self::PrintSides => "Sides".to_owned(),
            Self::PrintSidesIs(pdf_print::service::Sides::One) => "One-sided".to_owned(),
            Self::PrintSidesIs(pdf_print::service::Sides::TwoLongEdge) => {
                "Two-sided, flip on long edge".to_owned()
            }
            Self::PrintSidesIs(pdf_print::service::Sides::TwoShortEdge) => {
                "Two-sided, flip on short edge".to_owned()
            }
            Self::PrintPaperMissing { printer, paper } => {
                format!("{printer} has no {paper} paper.")
            }
            Self::PrintEdgeTooNarrow {
                printer,
                millimetres,
            } => format!(
                "{printer} can't print within {millimetres} mm of the edge, and the margins \
                 are narrower: what is there will be cut off."
            ),
            Self::PrintPreparing { done, total } => {
                format!("Preparing sheet {done} of {total}\u{2026}")
            }
            Self::PrintSent {
                sheets: 1,
                printer,
                job,
            } => format!("Sent 1 sheet to {printer}{}", job.map_or_else(String::new, |job| format!(" (job {job})"))),
            Self::PrintSent {
                sheets,
                printer,
                job,
            } => format!("Sent {sheets} sheets to {printer}{}", job.map_or_else(String::new, |job| format!(" (job {job})"))),
            Self::PrintFailed(why) => format!("Not printed: {why}"),
            Self::PrintRefused => "This document's security settings don't allow printing.".to_owned(),
            Self::PrintDegraded => "This document allows only low-quality printing, so it \
                                    prints as a 150 dpi picture."
                .to_owned(),
            Self::PrintDrawing => "Drawing the sheet\u{2026}".to_owned(),
            Self::PrintLayout(pdf_print::LayoutError::NoPages) => "No pages to print".to_owned(),
            Self::PrintLayout(pdf_print::LayoutError::BadGrid) => {
                "Pages per sheet must be at least 1 by 1".to_owned()
            }
            Self::PrintLayout(pdf_print::LayoutError::BadSize) => {
                "A size or scale is not a positive number".to_owned()
            }
            Self::PrintLayout(pdf_print::LayoutError::NoRoom) => {
                "The margins leave no room on the paper".to_owned()
            }
            Self::PrintSheetFailed(why) => format!("This sheet can't be drawn: {why}"),
            Self::NoGraphics(said) => format!(
                "PanPDF can't draw on this screen.\n\nThis computer's graphics driver is too \
                 old, or none is installed yet. Install or update the display driver for this \
                 computer's graphics card, then start PanPDF again. If you are working on this \
                 computer from somewhere else, try again at the computer itself.\n\n{said}"
            ),
            Self::SplitWhy => "Each file holds a run of pages, and together they hold every \
                               page of this document."
                .to_owned(),
            Self::SplitEvery => "Files of".to_owned(),
            Self::SplitEveryPages => "pages each".to_owned(),
            Self::SplitAtPages => "Start a new file at pages".to_owned(),
            Self::SplitInto { pages: 1, files } => format!("1 page in {files} files"),
            Self::SplitInto { pages, files: 1 } => format!("{pages} pages in 1 file"),
            Self::SplitInto { pages, files } => format!("{pages} pages in {files} files"),
            Self::SplitWhere => "Choose where\u{2026}".to_owned(),
            Self::TakingPagesOut => "Copying the pages out\u{2026}".to_owned(),
            Self::WroteFiles {
                files,
                first,
                bytes,
            } => format!("Wrote {files} PDFs, from {first} ({bytes} bytes in all)"),
            Self::CouldNotTakePagesOut(why) => format!("The pages were not written out: {why}"),
            Self::ExportWhy => "Each page is written out as a PNG picture of what is on \
                                screen, one file for each page."
                .to_owned(),
            Self::ExportResolution => "Resolution".to_owned(),
            Self::ExportDpi => "dots an inch".to_owned(),
            Self::ExportSize {
                pages: 1,
                width,
                height,
            } => format!("1 page, {width} \u{d7} {height} pixels"),
            Self::ExportSize {
                pages,
                width,
                height,
            } => format!("{pages} pages, the first {width} \u{d7} {height} pixels"),
            Self::DrawingPictures => "Drawing the pages\u{2026}".to_owned(),
            Self::WrotePictures {
                files,
                first,
                bytes,
            } => format!("Wrote {files} pictures, from {first} ({bytes} bytes in all)"),
            Self::CouldNotWritePictures(why) => {
                format!("The pages were not written out as pictures: {why}")
            }
            Self::MakingPages => "Making pages of the pictures\u{2026}".to_owned(),
            Self::CouldNotMakePages(why) => {
                format!("The pictures were not made into pages: {why}")
            }
            Self::RemoveThePlace => "Remove".to_owned(),
            Self::RemoveThePlaceWhy => {
                "Links sent to this name will point nowhere afterwards".to_owned()
            }
            Self::LinkToAnotherDocument => "A page of another document".to_owned(),
            Self::LinkFileHint => "handbook.pdf, beside this document".to_owned(),
            Self::LinkNeedsAFile => "Type the name of the file to open".to_owned(),
            Self::LinkNeedsAPageNumber => "A page number is a whole number from one".to_owned(),
            Self::FieldText => "Text".to_owned(),
            Self::FieldParagraph => "Paragraph".to_owned(),
            Self::FieldCheckbox => "Checkbox".to_owned(),
            Self::FieldRadio => "Radio button".to_owned(),
            Self::FieldDropdown => "Dropdown".to_owned(),
            Self::FieldChoices => "Choices, separated by commas".to_owned(),
            Self::FieldGroup => "Group".to_owned(),
            Self::FieldNewGroup => "New group".to_owned(),
            Self::Contents => "Contents".to_owned(),
            Self::AddBookmarkSaid => "Add a bookmark for the page on screen".to_owned(),
            Self::RenameThePlace | Self::RenameBookmark => "Rename".to_owned(),
            Self::BookmarkUp => "Move up".to_owned(),
            Self::BookmarkDown => "Move down".to_owned(),
            Self::BookmarkIn => "Make it a child of the one above".to_owned(),
            Self::BookmarkOut => "Take it out of its branch".to_owned(),
            Self::BookmarkName => "Bookmark name".to_owned(),
            Self::FieldsPanel => "Fields".to_owned(),
            Self::NoFieldsYet => "This document has no form fields yet".to_owned(),
            Self::OrderByRow => "Rows".to_owned(),
            Self::OrderByColumn => "Columns".to_owned(),
            Self::OrderByRowSaid => "Tab through this page's fields along its rows".to_owned(),
            Self::OrderByColumnSaid => "Tab through this page's fields down its columns".to_owned(),
            Self::FieldPointer => "Select fields".to_owned(),
            Self::ArrangeFields => "Arrange".to_owned(),
            Self::AlignLeftEdges => "Align left".to_owned(),
            Self::AlignRightEdges => "Align right".to_owned(),
            Self::AlignTops => "Align top".to_owned(),
            Self::AlignBottoms => "Align bottom".to_owned(),
            Self::AlignCentresAcross => "Align centres on a vertical line".to_owned(),
            Self::AlignCentresDown => "Align centres on a horizontal line".to_owned(),
            Self::DistributeAcross => "Distribute across".to_owned(),
            Self::DistributeDown => "Distribute down".to_owned(),
            Self::MatchWidth => "Match width".to_owned(),
            Self::MatchHeight => "Match height".to_owned(),
            Self::MatchSize => "Match width and height".to_owned(),
            Self::CentreAcrossPage => "Centre across the page".to_owned(),
            Self::CentreDownPage => "Centre down the page".to_owned(),
            Self::FieldProperties => "Properties".to_owned(),
            Self::FieldListBox => "List box".to_owned(),
            Self::FieldNone => "None".to_owned(),
            Self::FieldDate => "Date".to_owned(),
            Self::FieldSignature => "Signature".to_owned(),
            Self::FieldButton => "Button".to_owned(),
            Self::FieldCaption => "Caption".to_owned(),
            Self::FieldLink => "Opens this address".to_owned(),
            Self::FieldDateFormat => "Date format".to_owned(),
            Self::FieldNotADate => "This is not a date in the form the field asks for".to_owned(),
            Self::FieldNoLink => "This button opens nothing".to_owned(),
            Self::FieldGeneral => "General".to_owned(),
            Self::FieldAppearance => "Appearance".to_owned(),
            Self::FieldOptions => "Options".to_owned(),
            Self::FieldTooltip => "Tooltip".to_owned(),
            Self::FieldVisibility => "Form field".to_owned(),
            Self::FieldVisible => "Visible".to_owned(),
            Self::FieldHidden => "Hidden".to_owned(),
            Self::FieldVisibleNotPrinted => "Visible but doesn't print".to_owned(),
            Self::FieldHiddenPrinted => "Hidden but printable".to_owned(),
            Self::FieldBorderColour => "Border colour".to_owned(),
            Self::FieldFillColour => "Fill colour".to_owned(),
            Self::FieldLineThickness => "Line thickness".to_owned(),
            Self::FieldThin => "Thin".to_owned(),
            Self::FieldMedium => "Medium".to_owned(),
            Self::FieldThick => "Thick".to_owned(),
            Self::FieldLineStyle => "Line style".to_owned(),
            Self::FieldSolid => "Solid".to_owned(),
            Self::FieldDashed => "Dashed".to_owned(),
            Self::FieldBeveled => "Beveled".to_owned(),
            Self::FieldInset => "Inset".to_owned(),
            Self::FieldDefaultValue => "Default value".to_owned(),
            Self::FieldMultiline => "Multi-line".to_owned(),
            Self::FieldPassword => "Password".to_owned(),
            Self::FieldScroll => "Scroll long text".to_owned(),
            Self::FieldSpellCheck => "Check spelling".to_owned(),
            Self::FieldLimit => "Limit of characters".to_owned(),
            Self::FieldComb => "Comb of characters".to_owned(),
            Self::FieldMarkStyle => "Check box style".to_owned(),
            Self::MarkCheck => "Check".to_owned(),
            Self::MarkCircle => "Circle".to_owned(),
            Self::MarkCross => "Cross".to_owned(),
            Self::MarkDiamond => "Diamond".to_owned(),
            Self::MarkSquare => "Square".to_owned(),
            Self::MarkStar => "Star".to_owned(),
            Self::FieldExportValue => "Export value".to_owned(),
            Self::FieldCheckedByDefault => "Checked by default".to_owned(),
            Self::FieldInUnison => "Buttons with the same name and choice are selected in unison".to_owned(),
            Self::FieldItem => "Item".to_owned(),
            Self::FieldAddItem | Self::AiAdd => "Add".to_owned(),
            Self::FieldDeleteItem => "Delete".to_owned(),
            Self::FieldItemUp => "Up".to_owned(),
            Self::FieldItemDown => "Down".to_owned(),
            Self::FieldSort => "Sort items".to_owned(),
            Self::FieldCustomText => "Allow user to enter custom text".to_owned(),
            Self::FieldMultiSelect => "Multiple selection".to_owned(),
            Self::FieldCommitAtOnce => "Commit selected value immediately".to_owned(),
            Self::FieldNoOptions => "This kind of field has no options here yet".to_owned(),
            Self::FieldSettings => "Field settings".to_owned(),
            Self::FieldName => "Name".to_owned(),
            Self::FieldRequired => "Must be filled in".to_owned(),
            Self::FieldReadOnly => "Read only".to_owned(),
            Self::FieldAlignment => "Alignment".to_owned(),
            Self::AlignLeft => "Left".to_owned(),
            Self::AlignCentre => "Centre".to_owned(),
            Self::AlignRight => "Right".to_owned(),
            Self::FieldTextSize => "Text size".to_owned(),
            Self::FieldAutoSize => "Fit the box".to_owned(),
            Self::Apply => "Apply".to_owned(),
            Self::DeleteField => "Delete field".to_owned(),
            Self::SignatureFieldsAreNotSignedYet => {
                "Signature fields cannot be signed yet; put a signature on the page instead"
                    .to_owned()
            }
            Self::HowToFinishAField => {
                "Enter or Tab to keep what you typed, Esc to leave the field as it was".to_owned()
            }
            Self::Command(command) => match command {
                Command::Open => "Open",
                Command::Save => "Save",
                Command::Undo => "Undo",
                Command::Redo => "Redo",
                Command::Copy => "Copy",
                Command::Cut => "Cut",
                Command::Paste => "Paste",
                Command::PasteInPlace => "Paste in place",
                Command::Select => "Select",
                Command::Text => "Text",
                Command::Find => "Find in document",
                Command::Replace => "Replace",
                Command::ReplaceWith => "Replace with",
                Command::ReplaceThisOne => "Replace this one",
                Command::ReplaceAll => "Replace all…",
                Command::FindNext => "Next result",
                Command::FindPrevious => "Previous result",
                Command::Pen => "Pen",
                Command::Highlighter => "Highlighter",
                Command::Shape => "Shape",
                Command::Form => "Form",
                Command::Link => "Link",
                Command::Contents => "Contents",
                Command::Picture => "Picture",
                Command::Delete => "Delete",
                Command::BringToFront => "Bring to front",
                Command::BringForward => "Bring forward",
                Command::SendBackward => "Send backward",
                Command::SendToBack => "Send to back",
                Command::PreviousPage => "Previous page",
                Command::NextPage => "Next page",
                Command::ZoomIn => "Zoom in",
                Command::ZoomOut => "Zoom out",
                Command::NewDocument => "New document",
                Command::AllowEditing => "Allow editing…",
                Command::Theme => "Light or dark",
                Command::File => "File",
                Command::Edit => "Edit",
                Command::View => "View",
                Command::Documents => "Documents in this folder",
                Command::ShowFrames => "Show frames",
                Command::Frames => "Frames",
                Command::FramesOfText => "Around text",
                Command::FramesOfPictures => "Around pictures",
                Command::FramesOfDrawings => "Around drawings (lines and shapes)",
                Command::DarkMode => "Dark mode",
                Command::ShowDrawingSpeed => "Show drawing speed",
                Command::Language => "Language",
                Command::Help => "Help",
                Command::ReportAProblem => "Report a problem...",
                Command::ShowTheLog => "Show the log",
                Command::Pages => "Pages",
                Command::HidePages => "Hide the pages",
                Command::BackToReading => "Back to the document",
                Command::Home => "Home",
                Command::Page => "Page",
                Command::BlankPageBefore => "Insert blank page before",
                Command::BlankPageAfter => "Insert blank page after",
                Command::SameSizeAsThisPage => "Same size as this page",
                Command::Landscape => "Landscape",
                Command::DeletePage => "Delete this page",
                Command::MovePageEarlier => "Move page up",
                Command::MovePageLater => "Move page down",
                Command::MovePageFirst => "Move page to the start",
                Command::MovePageLast => "Move page to the end",
                Command::RotateClockwise => "Rotate right",
                Command::RotateCounterClockwise => "Rotate left",
                Command::StampPageNumbers => "Page numbers\u{2026}",
                Command::StampHeaderFooter => "Header & footer\u{2026}",
                Command::StampWatermark => "Watermark\u{2026}",
                Command::RecognizeText => "Recognize text (OCR)\u{2026}",
                Command::AiAssistant => "AI assistant\u{2026}",
                Command::ConnectAgents => "Connect agents\u{2026}",
                Command::Print => "Print\u{2026}",
                Command::PagesFromFileBefore => "Insert pages from a file before…",
                Command::PagesFromFileAfter => "Insert pages from a file after…",
                Command::SaveAs => "Save a copy as…",
                Command::DuplicatePage => "Duplicate these pages",
                Command::PagesToNewFile => "Save these pages as a new PDF…",
                Command::SplitDocument => "Split into several PDFs…",
                Command::PagesAsPictures => "Export pages as pictures…",
                Command::PdfFromPictures => "Create a PDF from pictures…",
                Command::PicturesAsPagesBefore => "Pictures as pages before these…",
                Command::PicturesAsPagesAfter => "Pictures as pages after these…",
                Command::Insert => "Insert",
                Command::Tools => "Tools",
                Command::InsertPictures => "Pictures…",
                Command::InsertText => "Text box",
                Command::InsertField => "Form field",
                Command::InsertBlankPage => "Blank page",
                Command::InsertPagesFromPdf => "Pages from a PDF\u{2026}",
                Command::InsertPicturesAsPages => "Pictures as pages\u{2026}",
                Command::InsertHere => "Put pages in here",
                Command::AddPages => "Add pages",
                Command::MoreForPage => "More\u{2026}",
            }
            .to_owned(),
            Self::PageOf { page, count } => format!("{page} of {count}"),
            Self::ZoomPercent(zoom) => format!("{zoom}%"),
            Self::Opening(name) => format!("Opening {name}"),
            Self::OpeningFailed => "The document failed to open unexpectedly".to_owned(),
            Self::DocumentCount(count) => format!("{count} documents"),
            Self::ObjectCount(count) => format!("{count} objects"),
            Self::StartedANewDocument => {
                "A new document of one blank page. Save it to give it a file.".to_owned()
            }
            Self::SavedTo { name, bytes } => format!("Saved to {name} ({bytes} bytes)"),
            Self::CouldNotSave { name, why } => format!("{name} could not be saved: {why}"),
            Self::PagesNotRead { name, why } => {
                format!("The pages of {name} could not be read: {why}")
            }
            Self::PictureNotRead { name, why } => {
                format!("The picture {name} could not be read: {why}")
            }
            Self::DraftCopied => {
                format!("Draft copied{SEP}it stays here until you discard it")
            }
            Self::DraftDiscarded => "Draft discarded".to_owned(),
            Self::PageWillNotOpen { page, why } => format!("Page {page} will not open\n{why}"),
            Self::FrameDeclared { wide, high, relaid } => {
                let done = if *relaid {
                    "text laid out again"
                } else {
                    "text not laid out again"
                };
                format!("Frame {wide:.0} by {high:.0} points \u{2014} declared, {done}")
            }
            Self::FrameKeptNotRelaid { wide, high, why } => format!(
                "Frame {wide:.0} by {high:.0} points \u{2014} kept at its new width, but the \
                 text could not be laid out again: {why}"
            ),
            Self::ParagraphOf { rows, runs } => {
                format!("Paragraph of {rows} rows{SEP}{runs} show operations")
            }
            Self::ObjectSized { kind, wide, high } => {
                format!("{} {wide:.0} by {high:.0} points", kind.english())
            }
            Self::Selected(count) => format!("{count} selected"),
            Self::ObjectKind(kind) => kind.english().to_owned(),
            Self::WentToPage(page) => format!("Went to page {page}"),
            Self::LinkLeadsOffTheDocument => {
                "This link leads to a page the document does not have".to_owned()
            }
            Self::WillNotOpenFromDocument(uri) => {
                format!("This kind of address is not opened from inside a document: {uri}")
            }
            Self::CouldNotOpen { uri, why } => format!("{uri} could not be opened: {why}"),
            Self::Opened(uri) => format!("Opened {uri}"),
            Self::LinkKindNotOpenable => "This kind of link cannot be opened from here".to_owned(),
            Self::PageSaid { page, said } => format!("Page {page}: {said}"),
            Self::AndPressesRefused { said, refused } => format!(
                "{}{SEP}{refused} further key presses could not follow an edit that did not land",
                said.say(Lang::English)
            ),
            Self::DoubleClickToTypeHere => {
                "Double-click a paragraph to put the caret in it, then type".to_owned()
            }
            Self::DraftIsCollectingWhatYouType => format!(
                "A draft is waiting{SEP}what you type joins it (retry, copy or discard)"
            ),
            Self::RangeCannotBeCopied => {
                format!("This range cannot be copied{SEP}the paragraph's text does not read")
            }
            Self::Copied(count) => format!("{count} characters copied"),
            Self::SelectionOf {
                shown,
                clusters,
                unreadable,
            } => {
                let head = format!("\u{201c}{shown}\u{201d} selected, {clusters} clusters");
                if *unreadable == 0 {
                    head
                } else {
                    format!(
                        "{head}\n\n{unreadable} of them are not in the font's /ToUnicode table, so what they say cannot be read (shown as \u{fffd}){SEP}moving and deleting still work"
                    )
                }
            }
            Self::SelectionOfUnread(clusters) => format!("{clusters} clusters selected"),
            Self::PressDeleteToRemove => "\u{00b7} press Delete to remove".to_owned(),
            Self::DeleteBlockHelp => "Delete this block (Del)".to_owned(),
            Self::DeletePlannableHere => {
                "The selection names its source spans in full, so one step can remove it".to_owned()
            }
            Self::DeleteNotPlannableHere => {
                "The selection has no unbroken run of source, or it crosses streams that cannot be edited together \u{2014} try a shorter one"
                    .to_owned()
            }
            Self::EditingText => "Editing text".to_owned(),
            Self::EditingTextHelp => {
                "Drag to select. The arrow keys walk the caret inside this block. Click the paper to leave."
                    .to_owned()
            }
            Self::DragToSelect => "\u{00b7} drag to select".to_owned(),
            Self::BlockOfRuns { runs, .. } => format!("\u{00b7} {runs} show operations"),
            Self::BlockOfRunsHelp { runs, lines } => format!(
                "This block is written as {runs} show operations over {lines} rows, and all of them move together in one step\nDrag to move the block \u{00b7} click again to edit the text inside"
            ),
            Self::Size => "Size".to_owned(),
            Self::SizeOfSelectionHelp => {
                "The selection's text size, in points on the page\nType, then press Enter".to_owned()
            }
            Self::SizeOfBlockHelp => {
                "The text size in points on the page, not the number in the file's Tf \u{2014} text scaled by a matrix shows the size an eye sees\nType, then press Enter"
                    .to_owned()
            }
            Self::LineSpacing => "Line spacing".to_owned(),
            Self::LineSpacingHelp => {
                "From one row's baseline to the next, in points \u{00b7} applies to the whole frame\nThe frame grows or shrinks with it; what is below does not move\nType, then press Enter"
                    .to_owned()
            }
            Self::AlignStartHelp => {
                "Align left: every line starts at the frame's left edge".to_owned()
            }
            Self::AlignCentreHelp => {
                "Centre: every line sits in the middle of the frame".to_owned()
            }
            Self::AlignEndHelp => {
                "Align right: every line ends at the frame's right edge".to_owned()
            }
            Self::AlignJustifyHelp => {
                "Justify: fill both edges of the frame\nThe spare width is spread where a line may end \u{2014} between words, and between clusters in a script that has no spaces"
                    .to_owned()
            }
            Self::FlowRoundHelp => {
                "Flow round pictures and drawings: the lines keep out of what stands in the frame, round its upright box\nThe frame keeps its width where it is free and gives up the rest"
                    .to_owned()
            }
            Self::FlowsRoundNow => "The text now flows round what stands in it".to_owned(),
            Self::NothingInTheWay => "Nothing stands in this block's way".to_owned(),
            Self::FlowRoundFellBehind => {
                "The text could not be laid out again round what stands in it, and still flows round where that was"
                    .to_owned()
            }
            Self::TextFlowsRoundThis => {
                "Let the text flow round this\nThe text keeps clear of its upright box, turned or not"
                    .to_owned()
            }
            Self::OrderingWaitsForThePage => "This page has not read back yet".to_owned(),
            Self::TextMadeWay { blocks } if *blocks == 1 => {
                "One block of text moved out of its way".to_owned()
            }
            Self::TextMadeWay { blocks } => {
                format!("{blocks} blocks of text moved out of its way")
            }
            Self::Times => "times".to_owned(),
            Self::TimesTextSize { ratio, points } => {
                format!("{ratio:.2} times the text size ({points:.1} points)")
            }
            Self::Bold => "Bold".to_owned(),
            Self::BoldHelp => "Bold: the bold face of the same family".to_owned(),
            Self::Italic => "Italic".to_owned(),
            Self::ItalicHelp => "Italic: the italic face of the same family".to_owned(),
            Self::Underline => "Underline".to_owned(),
            Self::NoUnderline => "Remove the underline".to_owned(),
            Self::PlainFace => "Plain".to_owned(),
            Self::PlainFaceHelp => "Plain: neither bold nor italic".to_owned(),
            Self::Font => "Font".to_owned(),
            Self::FontHelp => {
                "The font of the selection, or of the whole block when a block is selected".to_owned()
            }
            Self::GrowFont => "Increase font size".to_owned(),
            Self::ShrinkFont => "Decrease font size".to_owned(),
            Self::ClearFormatting => "Clear formatting: plain face, no underline".to_owned(),
            Self::TextColour => "Font colour".to_owned(),
            Self::StyleForNextTyping => "The next text you type here will use this".to_owned(),
            Self::NothingToFormat => {
                "Select a block, or drag over text inside one, to format it".to_owned()
            }
            Self::PaintSelection(name) => format!("Paint the selection {name}"),
            Self::Turn => "Turn".to_owned(),
            Self::TurnHelp => {
                "The angle the text's baseline faces, in degrees, anticlockwise\nType, then press Enter"
                    .to_owned()
            }
            Self::Slant => "Slant".to_owned(),
            Self::SlantHelp => {
                "The angle the letters' uprights lean, in degrees \u{2014} a shear of the matrix, not an italic face (rfcs/0008: slant is not italic)\nType, then press Enter"
                    .to_owned()
            }
            Self::CaretGoneBeforeStyling => {
                format!("The caret is no longer in text{SEP}click the paragraph again before styling")
            }
            Self::CaretGoneBeforeTyping => {
                format!("The caret is no longer in text{SEP}click the paragraph again before typing")
            }
            Self::CaretGoneBeforeDeleting => {
                format!("The caret is no longer in text{SEP}click the paragraph again before deleting")
            }
            Self::BlockTextNotFound => {
                format!("This block's text could not be found{SEP}click the paragraph again before styling")
            }
            Self::GroupLetGoAfterTheEdit => {
                format!("Not everything that was chosen is still there{SEP}the group was let go, so drag a band round it again")
            }
            Self::DraftBarTitle => "Not in the document yet:".to_owned(),
            Self::DraftRetry => "Retry".to_owned(),
            Self::DraftCopy => "Copy".to_owned(),
            Self::DraftDiscard => "Discard".to_owned(),
            Self::DraftWaitForTheEditToLand => "Wait for the running edit to land".to_owned(),
            Self::DraftPlaceNoLongerUsable => {
                "Its place cannot be used any more \u{2014} copy it and paste it yourself"
                    .to_owned()
            }
            Self::AiTitle => "AI assistant".to_owned(),
            Self::AiBaseUrl => "Base URL".to_owned(),
            Self::AiApiKey => "API key (session only)".to_owned(),
            Self::AiModel => "Model".to_owned(),
            Self::AiProvider => "Provider".to_owned(),
            Self::AiAttach => "Attach a picture or a PDF".to_owned(),
            Self::AiHistory => "Earlier chats".to_owned(),
            Self::AiNoChatsYet => "No chats kept yet".to_owned(),
            Self::AiUntitledChat => "Untitled chat".to_owned(),
            Self::AiForgetChat => "Delete this chat".to_owned(),
            Self::AiKeepTheKey => "Keep this key on this machine".to_owned(),
            Self::AiKeepTheKeyMeans => "The key is written to a file only you can read, \
                 locked with a secret made for this installation. It keeps the key out of \
                 backups and screenshots; it does not protect it from a program already \
                 running as you. Leave it unticked to give the key afresh each session."
                .to_owned(),
            Self::AiNowOnDocument(name) => format!("Now on {name}"),
            Self::AiResetToDefault => "Reset to default".to_owned(),
            Self::AiFullAccessAsk => "Turn on full access?".to_owned(),
            Self::AiFullAccessMeans => "The assistant will change the document and take pages \
                 out of any file on this computer without asking you first. Every change is \
                 still one step you can undo. You can turn this off again at any time."
                .to_owned(),
            Self::AiFullAccessConfirm => "Turn it on".to_owned(),
            Self::AiAttachFiles => "Pictures or PDFs".to_owned(),
            Self::AiAttachFilesMeans => "Choose files to send with the question".to_owned(),
            Self::AiEffortOffMeans => "Leave it to the model".to_owned(),
            Self::AiEffortNoneMeans => "Answer straight away; fastest".to_owned(),
            Self::AiEffortLowMeans => "A little thought first".to_owned(),
            Self::AiEffortMediumMeans => "Think it through; slower".to_owned(),
            Self::AiEffortHighMeans => "Think hard; slowest, uses the most".to_owned(),
            Self::AiKeptKeyUnreadable => "The key kept on this machine could not be read. \
                 Give it again."
                .to_owned(),
            Self::AiTestConnection => "Test connection".to_owned(),
            Self::AiCancel => "Stop asking".to_owned(),
            Self::AiDisconnect => "Disconnect".to_owned(),
            Self::AiConnected => "Connected".to_owned(),
            Self::AiConnectedTo => "Connected \u{2713}".to_owned(),
            Self::AiNotConnected => "Not connected \u{2014} open the settings and give a key".to_owned(),
            Self::AiCheckingConnection => "Checking the connection\u{2026}".to_owned(),
            Self::AiNotConnectedYet => "Not connected \u{2014} open the settings".to_owned(),
            Self::AiThisDocumentsChat => "This document's chat, carried on from last time".to_owned(),
            Self::AiEdit => "Edit".to_owned(),
            Self::AiEditMeans => "Go back to this question to change it and ask again".to_owned(),
            Self::AiAskAgain => "Ask again".to_owned(),
            Self::AiAskAgainMeans => "Ask for this answer again".to_owned(),
            Self::AiWentBack => "Went back to an earlier question.".to_owned(),
            Self::AiWentBackDocumentStays => {
                "What the assistant changed in the document is still there \u{2014} Undo takes it back.".to_owned()
            }
            Self::AiPutBack => "Put the conversation back".to_owned(),
            Self::AiChatMenu => "More".to_owned(),
            Self::AiCopyWholeChat => "Copy the whole chat".to_owned(),
            Self::AiDeleteThisChatSure => "Click again to delete it for good".to_owned(),
            Self::AiThisDocument => "This document".to_owned(),
            Self::AiOtherChats => "Other chats".to_owned(),
            Self::AiQuestionForYou => "A question for you".to_owned(),
            Self::AiOwnAnswer => "Or type your own answer\u{2026}".to_owned(),
            Self::AiAnswer => "Answer".to_owned(),
            Self::AiSkipQuestion => "Skip".to_owned(),
            Self::AiWritingTheAnswer => "Writing the answer\u{2026}".to_owned(),
            Self::AiWaitingForModel(model) => format!("Waiting for {model}\u{2026}"),
            Self::AiWritingPieces { written, pieces } => {
                format!("Writing the document \u{2014} {written} of {pieces}")
            }
            Self::AiAskHint => "Ask a question\u{2026}".to_owned(),
            Self::AiIncludeContext { characters } => {
                format!("Include the text of the page on screen (up to {characters} characters)")
            }
            Self::AiSend => "Send".to_owned(),
            Self::AiResponse => "Response".to_owned(),
            Self::AiCopyResponse => "Copy response".to_owned(),
            Self::AiPrivacy => {
                "Finding models asks for the list and nothing else. The key stays in \
                 memory and is never written to disc. In Chat only, what leaves this \
                 machine is your question, and this page's text if you tick it. In the \
                 other modes the assistant may also send what it reads of the document \
                 -- its text, a search, its properties, a page as a picture -- and \
                 nothing else. Nothing is sent unless you ask a question."
                    .to_owned()
            }
            Self::AiWorkerStopped => {
                "The worker asking the provider stopped without an answer".to_owned()
            }
            Self::AiNothingAskedYet => {
                "Ask about the document on screen, or anything else.".to_owned()
            }
            Self::AiNewChat | Self::AiNewChatTitle => "New chat".to_owned(),
            Self::AiConnection => "Connection".to_owned(),
            Self::AiYou => "You".to_owned(),
            Self::AiThinking => "Thinking\u{2026}".to_owned(),
            Self::AiKeyNeeded => "This provider needs a key".to_owned(),
            Self::AiFindModels => "Find models".to_owned(),
            Self::AiNoModelChosen => "Choose a model".to_owned(),
            Self::AiEnterSends => "Enter sends \u{00b7} Shift+Enter for a new line".to_owned(),
            Self::AiSendThePage => "Send this page".to_owned(),
            Self::AiConnectionInvalid => {
                format!("This connection cannot be used as it is{SEP}check the address and the model")
            }
            Self::AiStopped => "The question was stopped".to_owned(),
            Self::AiCouldNotReach => {
                format!("This provider could not be reached{SEP}check the address, and that it is running")
            }
            Self::AiAnswerTooLarge => "The answer was too large to read".to_owned(),
            Self::AiServiceRefused => {
                format!("The provider refused the question{SEP}what it said is below")
            }
            Self::AiAnswerUnexpected => {
                "The provider answered something this program cannot read as an answer".to_owned()
            }
            Self::AiDismiss => "Dismiss".to_owned(),
            Self::AiWantsTo => "The assistant would like to".to_owned(),
            Self::AiAllowOnce => "Allow once".to_owned(),
            Self::AiAllowForThisChat => "Allow for this chat".to_owned(),
            Self::AiRefuse => "Refuse".to_owned(),
            Self::AiChangedTheDocument { page } => {
                format!("The assistant changed page {page}. Undo takes it back.")
            }
            Self::AiTooManyRounds => {
                "The assistant asked for too many actions for one question and was stopped. \
                 Ask again, in smaller steps."
                    .to_owned()
            }
            Self::AiEffort => "Thinking".to_owned(),
            Self::AiEffortOff => "Thinking: default".to_owned(),
            Self::AiEffortNone => "Thinking: off".to_owned(),
            Self::AiEffortLow => "Thinking: low".to_owned(),
            Self::AiEffortMedium => "Thinking: medium".to_owned(),
            Self::AiEffortHigh => "Thinking: high".to_owned(),
            Self::AiMode => "What it may do".to_owned(),
            Self::AiModeChatOnly => "Chat only".to_owned(),
            Self::AiModeAskBeforeChanges => "Ask for approval".to_owned(),
            Self::AiModeDoIt => "Approve for me".to_owned(),
            Self::AiModeFree => "Full access".to_owned(),
            Self::AiModeChatOnlyWhat => {
                "It answers questions and cannot touch the document.".to_owned()
            }
            Self::AiModeAskBeforeChangesWhat => {
                "Reads freely; asks you before every change".to_owned()
            }
            Self::AiModeDoItWhat => {
                "Changes without asking; asks only before taking pages from another file"
                    .to_owned()
            }
            Self::AiModeFreeWhat => {
                "Never asks, even to read other files on this computer".to_owned()
            }
            Self::AgentsTitle => "Connect agents".to_owned(),
            Self::AgentsWhat => {
                "This program ships `panpdf-mcp`: a server that gives an AI agent \
                 the same PDF engine as this window, over the Model Context \
                 Protocol. An agent reads a document exactly, changes it through \
                 the same proved plans, and saves to a file it names."
                    .to_owned()
            }
            Self::AgentsForClaudeCode => "For Claude Code:".to_owned(),
            Self::AgentsForClients => {
                "For Claude Desktop and other MCP clients:".to_owned()
            }
            Self::AgentsNote => {
                "The server speaks over standard input and output, and never \
                 touches a document this window has open. When it is not beside \
                 this program, build it from the same source with \
                 `cargo build --release -p pdf-agent`."
                    .to_owned()
            }
            Self::AgentsCopied => "Copied".to_owned(),
            Self::Done(done) => done.say(Lang::English),
            Self::Home(home) => home.say(Lang::English),
            Self::Control(control) => control.say(Lang::English),
            Self::Refused(refusal) => refusal.say(Lang::English),
            Self::Quiet => String::new(),
            Self::DrawingSpeed(speed) => format!(
                "median {:.1} ms  ·  95th {:.1} ms  ·  worst {:.1} ms  ·  {} of {} over {:.1} ms",
                speed.median,
                speed.ninety_fifth,
                speed.worst,
                speed.slow,
                speed.frames,
                crate::speed::A_FRAME_MS,
            ),
            Self::EditSpeed(edit) => format!(
                "last edit {:.0} ms  ·  reading {:.0}  ·  planning {:.0}  ·  writing {:.0}  ·  the rest {:.0}",
                edit.total,
                edit.read,
                edit.plan,
                edit.write,
                edit.rest(),
            ),
            Self::Plain(said) => said.clone(),
            Self::DropToAttach => "Let go here to attach them to your question".to_owned(),
            Self::AiAttachPicture => "picture".to_owned(),
            Self::AiAttachPdf => "PDF".to_owned(),
            Self::AiAttachText => "text".to_owned(),
            Self::AiAttachRefused => "refused".to_owned(),
            Self::AiThinkingAloud => "thinking\u{2026}".to_owned(),
            Self::AiCopyCode => "Copy this code".to_owned(),
            Self::AiAPicture => "a picture".to_owned(),
            Self::AiAnswerCutShort => {
                "The model ran out of room and stopped here \u{2014} ask it to go on".to_owned()
            }
        }
    }
}

impl std::fmt::Display for Message {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.say(Lang::English))
    }
}

impl From<Home> for Message {
    fn from(home: Home) -> Self {
        Self::Home(home)
    }
}

impl From<Done> for Message {
    fn from(done: Done) -> Self {
        Self::Done(done)
    }
}

impl From<Refusal> for Message {
    fn from(refusal: Refusal) -> Self {
        Self::Refused(refusal)
    }
}

#[cfg(test)]
mod tests {
    use super::{Lang, LinkKind, Message, PrintScalingKind, megabytes};

    fn earlier() -> Vec<Message> {
        vec![
            Message::LinkToPage(7),
            Message::LinkToMissingPage,
            Message::LinkToAddress("https://example.org".to_owned()),
            Message::LinkToUnopenableAddress("file:///tmp/x".to_owned()),
            Message::LinkDoesSomethingElse(LinkKind::AttachedFile),
            Message::LinkPointsNowhere,
            Message::DraftBlocksDelete,
            Message::DraftTooLongForOneEdit {
                bytes: 9000,
                limit: 4096,
            },
            Message::NoDraftToRetry,
            Message::DraftWaitsForAnotherEdit,
            Message::DraftPlaceIsGone,
            Message::DraftPlaceIsUncertain,
            Message::OcrWhy,
            Message::OcrLanguages,
            Message::OcrLanguage("lao".to_owned()),
            Message::OcrSkipText,
            Message::OcrStart(4),
            Message::OcrStop,
            Message::OcrProgress { done: 2, total: 9 },
            Message::OcrNotInstalled,
            Message::OcrNoLanguage,
            Message::OcrModel,
            Message::OcrQuality(pdf_ocr::Quality::Accurate),
            Message::OcrQuality(pdf_ocr::Quality::Fast),
            Message::OcrErrorRate {
                code: "lao".to_owned(),
                cer: 9.0,
            },
            Message::OcrGetModel {
                code: "tha".to_owned(),
                bytes: 7_614_571,
            },
            Message::OcrGettingModel("eng".to_owned()),
            Message::OcrModelFailed("curl: (6)".to_owned()),
            Message::OcrGetEngine,
            Message::OcrGettingEngine,
            Message::OcrEngineFailed("apt-get: no".to_owned()),
            Message::OcrEngineElsewhere,
            Message::OcrThisPage,
            Message::SplitWhy,
            Message::SplitEvery,
            Message::SplitEveryPages,
            Message::SplitAtPages,
            Message::SplitInto { pages: 1, files: 1 },
            Message::SplitInto { pages: 9, files: 3 },
            Message::SplitWhere,
            Message::TakingPagesOut,
            Message::WroteFiles {
                files: 3,
                first: "book-1.pdf".to_owned(),
                bytes: 4096,
            },
            Message::CouldNotTakePagesOut("no room".to_owned()),
            Message::ExportWhy,
            Message::ExportResolution,
            Message::ExportDpi,
            Message::ExportSize {
                pages: 1,
                width: 2480,
                height: 3508,
            },
            Message::ExportSize {
                pages: 9,
                width: 2480,
                height: 3508,
            },
            Message::DrawingPictures,
            Message::WrotePictures {
                files: 3,
                first: "book-1.png".to_owned(),
                bytes: 4096,
            },
            Message::CouldNotWritePictures("no room".to_owned()),
            Message::MakingPages,
            Message::CouldNotMakePages("not a picture".to_owned()),
            Message::PrintWhy,
            Message::AllPages,
            Message::PrintCurrentPage,
            Message::SomePages,
            Message::PrintReverse,
            Message::PrintPaper,
            Message::PrintOrientation,
            Message::PrintOrientationIs(pdf_print::Orientation::Auto),
        ]
    }

    fn later() -> Vec<Message> {
        vec![
            Message::PrintOrientationIs(pdf_print::Orientation::Portrait),
            Message::PrintOrientationIs(pdf_print::Orientation::Landscape),
            Message::PrintPerSheet,
            Message::PrintCustomGrid,
            Message::PrintScaling(PrintScalingKind::Fit),
            Message::PrintScaling(PrintScalingKind::ShrinkOversized),
            Message::PrintScaling(PrintScalingKind::ActualSize),
            Message::PrintScaling(PrintScalingKind::Custom),
            Message::PrintOrder,
            Message::PrintOrderIs(pdf_print::Order::Horizontal),
            Message::PrintOrderIs(pdf_print::Order::HorizontalReversed),
            Message::PrintOrderIs(pdf_print::Order::Vertical),
            Message::PrintOrderIs(pdf_print::Order::VerticalReversed),
            Message::PrintBorders,
            Message::PrintAutoRotate,
            Message::PrintFitAgain,
            Message::PrintMargin,
            Message::PrintMarginOfPrinter {
                millimetres: "3.0".to_owned(),
            },
            Message::PrintSheet { at: 2, of: 5 },
            Message::PrintSummary {
                pages: 12,
                sheets: 3,
                paper: "A4".to_owned(),
            },
            Message::PrintButton,
            Message::PrintPrinter,
            Message::PrintNoPrinter,
            Message::PrintNoService("x".to_owned()),
            Message::PrintCopies,
            Message::PrintColour,
            Message::PrintMonochrome,
            Message::PrintSides,
            Message::PrintSidesIs(pdf_print::service::Sides::One),
            Message::PrintSidesIs(pdf_print::service::Sides::TwoLongEdge),
            Message::PrintSidesIs(pdf_print::service::Sides::TwoShortEdge),
            Message::PrintPaperMissing {
                printer: "P".to_owned(),
                paper: "A3".to_owned(),
            },
            Message::PrintEdgeTooNarrow {
                printer: "P".to_owned(),
                millimetres: "3.0".to_owned(),
            },
            Message::PrintPreparing { done: 3, total: 12 },
            Message::PrintSent {
                sheets: 12,
                printer: "P".to_owned(),
                job: Some(5),
            },
            Message::PrintSent {
                sheets: 1,
                printer: "P".to_owned(),
                job: None,
            },
            Message::PrintFailed("x".to_owned()),
            Message::PrintRefused,
            Message::PrintDegraded,
            Message::PrintDrawing,
            Message::PrintLayout(pdf_print::LayoutError::NoPages),
            Message::PrintLayout(pdf_print::LayoutError::BadGrid),
            Message::PrintLayout(pdf_print::LayoutError::BadSize),
            Message::PrintLayout(pdf_print::LayoutError::NoRoom),
            Message::PrintSheetFailed("x".to_owned()),
            Message::NoGraphics("wgpu: no adapter".to_owned()),
            Message::AiConnectionInvalid,
            Message::AiStopped,
            Message::AiCouldNotReach,
            Message::AiAnswerTooLarge,
            Message::AiServiceRefused,
            Message::AiAnswerUnexpected,
            Message::AiDismiss,
            Message::DropToAttach,
            Message::AiAttachPicture,
            Message::AiAttachPdf,
            Message::AiAttachText,
            Message::AiAttachRefused,
            Message::AiThinkingAloud,
        ]
    }

    fn newest() -> Vec<Message> {
        vec![
            Message::AiCheckingConnection,
            Message::AiNotConnectedYet,
            Message::AiThisDocumentsChat,
            Message::AiEdit,
            Message::AiEditMeans,
            Message::AiAskAgain,
            Message::AiAskAgainMeans,
            Message::AiWentBack,
            Message::AiWentBackDocumentStays,
            Message::AiPutBack,
            Message::AiChatMenu,
            Message::AiCopyWholeChat,
            Message::AiDeleteThisChatSure,
            Message::AiThisDocument,
            Message::AiOtherChats,
            Message::AiQuestionForYou,
            Message::AiOwnAnswer,
            Message::AiAnswer,
            Message::AiSkipQuestion,
            Message::AiWritingTheAnswer,
            Message::AiWaitingForModel("qwen".to_owned()),
            Message::AiWritingPieces {
                written: 3,
                pieces: 10,
            },
        ]
    }

    fn latest() -> Vec<Message> {
        vec![
            Message::AiCopyCode,
            Message::AiProvider,
            Message::AiAttach,
            Message::AiHistory,
            Message::AiNewChatTitle,
            Message::AiNoChatsYet,
            Message::AiUntitledChat,
            Message::AiForgetChat,
            Message::AiKeepTheKey,
            Message::AiKeepTheKeyMeans,
            Message::AiKeptKeyUnreadable,
            Message::AiNowOnDocument("a.pdf".to_owned()),
            Message::AiResetToDefault,
            Message::AiFullAccessAsk,
            Message::AiFullAccessMeans,
            Message::AiFullAccessConfirm,
            Message::AiAdd,
            Message::AiAttachFiles,
            Message::AiAttachFilesMeans,
            Message::AiEffortOffMeans,
            Message::AiEffortNoneMeans,
            Message::AiEffortLowMeans,
            Message::AiEffortMediumMeans,
            Message::AiEffortHighMeans,
            Message::AiAPicture,
            Message::AiAnswerCutShort,
            Message::AiEffortNone,
        ]
    }

    #[test]
    fn every_language_answers_every_message_and_answers_differently() {
        let every = [earlier(), later(), latest(), newest()].concat();
        for message in &every {
            for lang in Lang::ALL {
                let said = message.say(*lang);
                assert!(!said.trim().is_empty(), "{message:?} in {lang:?}");
            }
            if same_in_every_language(message) {
                continue;
            }
            for (at, first) in Lang::ALL.iter().enumerate() {
                for second in &Lang::ALL[at + 1..] {
                    assert_ne!(
                        message.say(*first),
                        message.say(*second),
                        "{message:?} says the same thing in {first:?} and {second:?}"
                    );
                }
            }
        }
    }

    fn same_in_every_language(message: &Message) -> bool {
        matches!(
            message,
            Message::ZoomPercent(_) | Message::Plain(_) | Message::Quiet | Message::OcrLanguage(_)
        )
    }

    #[test]
    fn a_message_that_is_the_same_everywhere_is_held_out_on_purpose() {
        for lang in Lang::ALL {
            assert_eq!(Message::ZoomPercent(160).say(*lang), "160%");
            assert_eq!(
                Message::Plain("xref 9 is past the end".to_owned()).say(*lang),
                "xref 9 is past the end"
            );
        }
        let page_of = Message::PageOf { page: 3, count: 7 };
        assert_eq!(page_of.say(Lang::English), "3 of 7");
        assert!(!same_in_every_language(&page_of));
    }

    #[test]
    fn no_two_languages_share_a_tag_or_a_name() {
        let mut tags: Vec<&str> = Lang::ALL.iter().map(|lang| lang.tag()).collect();
        let spoken = tags.len();
        tags.sort_unstable();
        tags.dedup();
        assert_eq!(tags.len(), spoken, "two languages share a tag");
        let mut names: Vec<&str> = Lang::ALL.iter().map(|lang| lang.endonym()).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), spoken, "two languages share a name");
        assert_eq!(spoken, Lang::ALL.len());
    }

    #[test]
    fn a_frame_declared_says_whether_it_was_laid_out_again() {
        let relaid = Message::FrameDeclared {
            wide: 40.0,
            high: 20.0,
            relaid: true,
        };
        let not = Message::FrameDeclared {
            wide: 40.0,
            high: 20.0,
            relaid: false,
        };
        assert!(relaid.say(Lang::English).contains("laid out again"));
        assert!(!relaid.say(Lang::English).contains("not laid out again"));
        assert!(not.say(Lang::English).contains("not laid out again"));
        assert_ne!(relaid.say(Lang::English), not.say(Lang::English));

        let empty = Message::FrameKeptNotRelaid {
            wide: 40.0,
            high: 20.0,
            why: "the block is empty".to_owned(),
        };
        let turned = Message::FrameKeptNotRelaid {
            wide: 40.0,
            high: 20.0,
            why: "the page is turned".to_owned(),
        };
        assert!(empty.say(Lang::English).contains("the block is empty"));
        assert_ne!(empty.say(Lang::English), turned.say(Lang::English));
    }

    #[test]
    fn a_messages_numbers_reach_the_sentence() {
        for lang in Lang::ALL {
            assert!(Message::LinkToPage(261).say(*lang).contains("261"));
            let long = Message::DraftTooLongForOneEdit {
                bytes: 9000,
                limit: 4096,
            };
            let said = long.say(*lang);
            assert!(said.contains("9000") && said.contains("4096"), "{said}");
            let read = Message::OcrProgress { done: 2, total: 9 }.say(*lang);
            assert!(read.contains('2') && read.contains('9'), "{read}");
            let sent = Message::PrintSent {
                sheets: 12,
                printer: "EPSON L3110".to_owned(),
                job: Some(57),
            }
            .say(*lang);
            assert!(
                sent.contains("12") && sent.contains("EPSON L3110") && sent.contains("57"),
                "{sent}"
            );
            let sent_without_job = Message::PrintSent {
                sheets: 12,
                printer: "EPSON L3110".to_owned(),
                job: None,
            }
            .say(*lang);
            assert!(
                sent_without_job.contains("12") && sent_without_job.contains("EPSON L3110"),
                "{sent_without_job}"
            );
            let preparing = Message::PrintPreparing { done: 3, total: 12 }.say(*lang);
            assert!(
                preparing.contains('3') && preparing.contains("12"),
                "{preparing}"
            );
            let sheet = Message::PrintSheet { at: 2, of: 5 }.say(*lang);
            assert!(sheet.contains('2') && sheet.contains('5'), "{sheet}");
            for (pages, sheets) in [(1, 1), (7, 1), (12, 3)] {
                let summary = Message::PrintSummary {
                    pages,
                    sheets,
                    paper: "Folio (F4)".to_owned(),
                }
                .say(*lang);
                assert!(
                    summary.contains(&pages.to_string())
                        && summary.contains(&sheets.to_string())
                        && summary.contains("Folio (F4)"),
                    "{summary}"
                );
            }
        }
    }

    #[test]
    fn a_size_is_read_in_megabytes() {
        assert_eq!(megabytes(0), "0.0 MB");
        assert_eq!(megabytes(1_048_576), "1.0 MB");
        assert_eq!(megabytes(1_048_575), "1.0 MB");
        assert_eq!(megabytes(1_572_864), "1.5 MB");
        assert_eq!(megabytes(6_386_744), "6.1 MB");
        assert_eq!(megabytes(13_532_551), "12.9 MB");
        assert_eq!(megabytes(15_400_601), "14.7 MB");
        assert_eq!(megabytes(7_614_571), "7.3 MB");
        assert_eq!(megabytes(1_072_600), "1.0 MB");
        assert_eq!(megabytes(4_113_088), "3.9 MB");
    }

    #[test]
    fn a_models_size_and_error_rate_reach_the_window() {
        for lang in Lang::ALL {
            for (quality, size, rate) in [
                (pdf_ocr::Quality::Accurate, "12.9 MB", "9.0"),
                (pdf_ocr::Quality::Fast, "6.1 MB", "11.6"),
            ] {
                let model = pdf_ocr::models::model("lao", quality).expect("a Lao model");
                let offer = Message::OcrGetModel {
                    code: model.code.to_owned(),
                    bytes: model.bytes,
                }
                .say(*lang);
                assert!(offer.contains(size), "{offer}");
                let said = Message::OcrErrorRate {
                    code: model.code.to_owned(),
                    cer: model.cer.expect("lao is one of the six measured rows"),
                }
                .say(*lang);
                assert!(said.contains(rate), "{said}");
            }
        }
    }

    #[test]
    fn a_language_tag_round_trips() {
        for lang in Lang::ALL {
            assert_eq!(Lang::of_tag(lang.tag()), Some(*lang));
            assert!(!lang.endonym().is_empty());
        }
        assert_eq!(Lang::of_tag("de"), None);
        assert_eq!(Lang::default(), Lang::English);
    }
}
