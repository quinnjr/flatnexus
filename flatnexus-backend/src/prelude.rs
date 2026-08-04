//! The subset of `armature::prelude::*` this crate actually uses.
//!
//! This crate depends on the individual armature crates rather than the
//! `armature-framework` meta-crate — see the note in `Cargo.toml` — so the
//! prelude that meta-crate exported has to be assembled here. Deliberately only
//! what is used: a re-export list padded with everything the upstream prelude
//! happened to carry is a list nobody prunes.

pub use armature_core::{Application, Error, HttpRequest, HttpResponse};

// `#[get]`, `#[post]` and `#[options]` are deliberately absent: `#[routes]`
// consumes those inner attributes while expanding the impl block, so they are
// never resolved as macros and importing them only produces dead re-exports.
pub use armature_proc_macro::{controller, module, routes};
