//! Domain layer of Quokka: plain data and rules about files, independent of the
//! file system, the operating system and the UI. Every other crate depends on this
//! one; it depends on nothing of ours.

pub mod entry;
pub mod format;
pub mod kind;
pub mod lang;
pub mod ops;
pub mod paths;
pub mod rename;
pub mod selection;
pub mod transfer;

pub use entry::{Entry, Filter, SortKey};
pub use kind::FileKind;
pub use ops::{OpKind, OpSpec};
