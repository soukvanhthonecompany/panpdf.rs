#![forbid(unsafe_code)]

pub mod addresses;
pub mod ai_choice;
pub mod ai_key;
pub mod ai_layout;
pub mod ai_permission;
pub mod ai_recall;
#[cfg(not(target_arch = "wasm32"))]
pub mod ai_status;
pub mod arrange;
pub mod dates;
pub mod document;
pub mod draft;
pub mod files;
pub mod find;
pub mod footprint;
pub mod frames;
pub mod ledger;
pub mod links;
pub mod live;
pub mod ocr_choice;
pub mod own_files;
pub mod painter;
pub mod pieces;
pub mod put_down;
pub mod recent;
pub mod speed;
pub mod strip;
pub mod tiles;
pub mod trouble;
pub mod view;
pub mod wording;

pub use document::{
    Applied, EditJob, EditOutcome, Editor, Export, Leaf, NewTextStyle, Overlay, RunBox,
};
pub use ledger::{CommandRecord, DocumentState, Ledger, Outcome};

#[cfg(test)]
mod cache_tests;
#[cfg(test)]
mod text_tests;
