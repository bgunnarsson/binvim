//! binvim library. Exists solely so the editor binary and the
//! `binvim-install` helper can share the install catalog + runner —
//! the editor's modules live in `src/main.rs` and aren't part of the
//! library, by design.

pub mod install;
