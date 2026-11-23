//! What the CLI does before it starts sharing or joining, given whether the
//! terms are stored, whether `--accept-terms` was passed, and whether a person
//! can answer (stdin is a terminal). The decision is in
//! `jafnify_core::ui::terms`; the terminal check stays in the CLI binary.

pub use jafnify_core::ui::terms::*;
