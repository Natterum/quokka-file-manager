//! Interface language. One process-wide setting read by the [`tr!`](crate::tr) and
//! [`trf!`](crate::trf) macros, so every crate can pick the right text without
//! threading the language through every call.

use std::sync::atomic::{AtomicU8, Ordering};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Lang {
    #[default]
    Ru,
    En,
}

impl Lang {
    pub const ALL: [Lang; 2] = [Lang::En, Lang::Ru];

    /// Key stored in the settings file.
    pub fn key(self) -> &'static str {
        match self {
            Lang::Ru => "ru",
            Lang::En => "en",
        }
    }

    pub fn from_key(s: &str) -> Option<Lang> {
        match s {
            "ru" => Some(Lang::Ru),
            "en" => Some(Lang::En),
            _ => None,
        }
    }

    /// Name of the language in that language, for the settings list.
    pub fn native_name(self) -> &'static str {
        match self {
            Lang::Ru => "Русский",
            Lang::En => "English",
        }
    }
}

static CURRENT: AtomicU8 = AtomicU8::new(0);

pub fn set(lang: Lang) {
    CURRENT.store(lang as u8, Ordering::Relaxed);
}

pub fn get() -> Lang {
    if is_en() { Lang::En } else { Lang::Ru }
}

#[inline]
pub fn is_en() -> bool {
    CURRENT.load(Ordering::Relaxed) == Lang::En as u8
}

/// `tr!("Русский", "English")` — the literal for the current interface language.
#[macro_export]
macro_rules! tr {
    ($ru:literal, $en:literal $(,)?) => {
        if $crate::lang::is_en() { $en } else { $ru }
    };
}

/// `trf!("Скопировано {n}", "Copied {n}", ...)` — `format!` with the format string
/// for the current interface language. Both strings must use the same arguments.
#[macro_export]
macro_rules! trf {
    ($ru:literal, $en:literal $(, $($arg:tt)*)?) => {
        if $crate::lang::is_en() {
            ::std::format!($en $(, $($arg)*)?)
        } else {
            ::std::format!($ru $(, $($arg)*)?)
        }
    };
}
