#![forbid(unsafe_code)]

pub mod app;

#[cfg(not(target_arch = "wasm32"))]
mod ai_actions;
#[cfg(not(target_arch = "wasm32"))]
mod ai_panel;
mod canvas;
mod chooser;
mod chrome;
mod clipboard;
mod contents;
mod draw_pen;
mod draw_shape;
mod drawing_speed;
mod field_properties;
mod fields_panel;
mod fill_form;
mod find_bar;
mod form_tool;
mod format;
mod hub;
pub use hub::pdfs_in;
mod icons;
mod input;
mod link_tool;
mod live_typing;
#[cfg(not(target_arch = "wasm32"))]
mod memory;
mod meter;
mod moment;
mod naming;
mod ocr_tool;
mod order;
mod page_actions;
mod page_motion;
mod pages;
mod palette;
mod pictures;
mod place_picture;
mod print_tool;
mod properties;
#[cfg(not(target_arch = "wasm32"))]
mod reporting;
mod room;
pub mod save_file;
mod stamp_tool;
#[cfg(not(target_arch = "wasm32"))]
pub mod startup;
mod system_dialog;
mod take_out;
mod text;
mod trace;
mod unlock;
mod window_state;

pub use window_state::ZOOMS;
