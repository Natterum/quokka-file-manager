//! Infrastructure layer of Quokka: adapters that implement the core ports on top of
//! `std::fs`, `notify` and the Windows shell, plus OS services the UI uses directly
//! (clipboard, drag and drop, icons) and engines with their own I/O (ZIP, search).

pub mod archive;
pub mod clipboard;
pub mod drives;
pub mod creds;
pub mod fsops;
pub mod ftp;
pub mod iconsrc;
pub mod net;
pub mod pick;
pub mod imgconv;
pub mod mtp;
pub mod rename;
pub mod search;
pub mod sftp;
#[cfg(test)]
mod sftp_test_server;
#[cfg(test)]
mod ftp_test_server;
pub mod shell_dnd;
pub mod shell_menu;
pub mod shell_new;
pub mod shell_props;
pub mod ssh_term;
pub mod stdfs;
pub mod update;
pub mod watcher;
pub mod xfer;
pub mod zip;

use quokka_core::ports::Services;
use quokka_core::Waker;
use std::sync::Arc;

/// The production adapters, ready to hand to tabs.
pub fn services(waker: Waker) -> Services {
    Services {
        fs: Arc::new(stdfs::StdFs),
        watcher: Some(Arc::new(watcher::NotifyWatcher)),
        waker,
    }
}
