//! Application layer of Quokka. Holds the use cases — a file-list tab, the queue of
//! background operations, folder size scans — and the ports (traits) through which
//! they reach the outside world: the file system, file operations, change
//! notifications and the UI's "repaint please" signal.
//!
//! This crate knows nothing about egui, Windows or `std::fs`; adapters live in
//! `quokka-infra`, and the UI wires them in at start-up.

pub mod folder_size;
pub mod job;
pub mod listing;
pub mod panel;
pub mod ports;
pub mod queue;
pub mod waker;

pub use quokka_domain as domain;
pub use waker::Waker;

#[cfg(test)]
pub(crate) mod fake;
