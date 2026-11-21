//! Group rename by mask (Ctrl+M): the mask language of Total Commander's «Multi-Rename
//! Tool», search and replace, case change and a counter — plus validation of the
//! result (bad characters, reserved names, duplicates inside the batch, names that are
//! already taken on disk). Pure rules: the file system is reached only through the
//! `exists` callback given to [`plan`].
//!
//! Mask elements (inside the name or extension mask):
//!
//! | Element | Meaning |
//! |---|---|
//! | `[N]` | name without extension; `[N2]` 2nd char, `[N2-5]` chars 2..5, `[N2,3]` 3 chars from the 2nd, `[N3-]` from the 3rd to the end, negative numbers count from the end: `[N-3-]` the last 3 chars |
//! | `[E]` | extension, with the same ranges |
//! | `[P]`, `[G]` | name of the parent / grandparent folder |
//! | `[C]` | counter; `[C10+5:3]` overrides start, step and digits |
//! | `[Y]` `[M]` `[D]` `[h]` `[m]` `[s]` | modification date and time; combinable: `[Y-M-D]`, `[hms]` |
//! | `[S]` | size in bytes |
//! | `[U]` `[L]` `[F]` `[n]` | following text: UPPER, lower, Each Word, unchanged |
//! | `[[]`, `[]]` | literal brackets |

use crate::{tr, trf};
use chrono::{DateTime, Datelike, Local, Timelike};
use regex::RegexBuilder;
use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// One item to rename.
#[derive(Clone, Debug)]
pub struct Source {
    /// Current path.
    pub path: PathBuf,
    pub is_dir: bool,
    pub size: u64,
    pub modified: Option<SystemTime>,
}

impl Source {
    pub fn name(&self) -> String {
        self.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
    }

    fn ancestor_name(&self, up: usize) -> String {
        self.path
            .ancestors()
            .nth(up)
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum CaseMode {
    #[default]
    Keep,
    Lower,
    Upper,
    /// First letter upper, the rest lower.
    First,
    /// First letter of every word upper, the rest lower.
    Words,
}

impl CaseMode {
    pub fn all() -> [(CaseMode, &'static str); 5] {
        [
        (CaseMode::Keep, tr!("Без изменений", "No change")),
        (CaseMode::Lower, tr!("все строчные", "all lowercase")),
        (CaseMode::Upper, tr!("ВСЕ ПРОПИСНЫЕ", "ALL UPPERCASE")),
        (CaseMode::First, tr!("Первая прописная", "First letter uppercase")),
        (CaseMode::Words, tr!("Каждое Слово С Прописной", "Every Word Capitalized")),
        ]
    }

    fn apply(self, s: &str) -> String {
        match self {
            CaseMode::Keep => s.to_string(),
            CaseMode::Lower => s.to_lowercase(),
            CaseMode::Upper => s.to_uppercase(),
            CaseMode::First => {
                let mut it = s.chars();
                match it.next() {
                    Some(c) => c.to_uppercase().chain(it.as_str().to_lowercase().chars()).collect(),
                    None => String::new(),
                }
            }
            CaseMode::Words => {
                let mut w = CaseWriter { mode: CaseMode::Words, out: String::new(), word: false };
                w.push(s);
                w.out
            }
        }
    }
}

/// Everything the user sets in the dialog.
#[derive(Clone, Debug, PartialEq)]
pub struct Rules {
    pub name_mask: String,
    pub ext_mask: String,
    /// Search text; in plain mode several alternatives may be separated by `|`
    /// (with the matching replacements also separated by `|`).
    pub find: String,
    pub replace: String,
    pub regex: bool,
    pub match_case: bool,
    pub case: CaseMode,
    pub start: i64,
    pub step: i64,
    pub digits: usize,
}

impl Default for Rules {
    fn default() -> Self {
        Rules {
            name_mask: "[N]".into(),
            ext_mask: "[E]".into(),
            find: String::new(),
            replace: String::new(),
            regex: false,
            match_case: false,
            case: CaseMode::Keep,
            start: 1,
            step: 1,
            digits: 1,
        }
    }
}

/// Why a new name can't be used.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Problem {
    Empty,
    BadChars,
    Reserved,
    TrailingDotOrSpace,
    TooLong,
    /// Two items of the batch would get the same name.
    Duplicate,
    /// A file or folder with this name already exists (and is not renamed away by this batch).
    Exists,
}

impl fmt::Display for Problem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Problem::Empty => tr!("пустое имя", "empty name"),
            Problem::BadChars => tr!("недопустимые символы \\ / : * ? \" < > |", "invalid characters \\ / : * ? \" < > |"),
            Problem::Reserved => tr!("зарезервированное имя Windows", "reserved Windows name"),
            Problem::TrailingDotOrSpace => tr!("имя не может кончаться точкой или пробелом", "name cannot end with a dot or a space"),
            Problem::TooLong => tr!("слишком длинное имя", "name is too long"),
            Problem::Duplicate => tr!("такое имя получат несколько объектов", "several items would get this name"),
            Problem::Exists => tr!("такое имя уже занято", "this name is already taken"),
        })
    }
}

/// Result for one item.
#[derive(Clone, Debug, PartialEq)]
pub struct Planned {
    pub new_name: String,
    /// The name differs from the current one (case-only changes count).
    pub changed: bool,
    pub problem: Option<Problem>,
}

/// Split a name into stem and extension (without the dot). Folders and names that
/// only start with a dot (`.gitignore`) have no extension.
pub fn split_name(name: &str, is_dir: bool) -> (&str, &str) {
    if is_dir {
        return (name, "");
    }
    match name.rfind('.') {
        Some(i) if i > 0 => (&name[..i], &name[i + 1..]),
        _ => (name, ""),
    }
}

// ───────────────────────────── mask ─────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq)]
enum End {
    Whole,
    One,
    ToEnd,
    To(i64),
    Len(usize),
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Range {
    start: i64,
    end: End,
}

impl Range {
    const WHOLE: Range = Range { start: 1, end: End::Whole };

    fn cut(&self, s: &str) -> String {
        let chars: Vec<char> = s.chars().collect();
        let n = chars.len() as i64;
        // 1-based from the start, or negative from the end (-1 = last char)
        let pos = |k: i64| if k > 0 { k - 1 } else { n + k };
        let a = pos(self.start);
        let b = match self.end {
            End::Whole => return s.to_string(),
            End::One => a,
            End::ToEnd => n - 1,
            End::To(e) => pos(e),
            End::Len(l) => a + l as i64 - 1,
        };
        let (a, b) = (a.max(0), b.min(n - 1));
        if a > b {
            return String::new();
        }
        chars[a as usize..=b as usize].iter().collect()
    }
}

#[derive(Clone, Debug, PartialEq)]
enum Tok {
    Lit(String),
    Name(Range),
    Ext(Range),
    Parent,
    Grand,
    Counter { start: Option<i64>, step: Option<i64>, digits: Option<usize> },
    Date(String),
    Size,
    Case(CaseMode),
}

/// Read an optionally negative integer from the front of `s`.
fn take_int(s: &str) -> Option<(i64, &str)> {
    let neg = s.starts_with('-');
    let body = if neg { &s[1..] } else { s };
    let len = body.bytes().take_while(u8::is_ascii_digit).count();
    if len == 0 {
        return None;
    }
    let v: i64 = body[..len].parse().ok()?;
    Some((if neg { -v } else { v }, &body[len..]))
}

fn parse_range(spec: &str, whole: &str) -> Result<Range, String> {
    let bad = || trf!("Непонятный диапазон «[{whole}]»", "Unclear range “[{whole}]”");
    if spec.is_empty() {
        return Ok(Range::WHOLE);
    }
    let (start, rest) = take_int(spec).ok_or_else(bad)?;
    if start == 0 {
        return Err(trf!("«[{whole}]»: символы нумеруются с 1", "“[{whole}]”: characters are numbered from 1"));
    }
    let end = if rest.is_empty() {
        End::One
    } else if let Some(len) = rest.strip_prefix(',') {
        End::Len(len.parse().map_err(|_| bad())?)
    } else if let Some(e) = rest.strip_prefix('-') {
        if e.is_empty() {
            End::ToEnd
        } else {
            match take_int(e) {
                Some((0, _)) => return Err(trf!("«[{whole}]»: символы нумеруются с 1", "“[{whole}]”: characters are numbered from 1")),
                Some((v, "")) => End::To(v),
                _ => return Err(bad()),
            }
        }
    } else {
        return Err(bad());
    };
    Ok(Range { start, end })
}

fn parse_counter(spec: &str, whole: &str) -> Result<Tok, String> {
    let bad = || trf!("Непонятный счётчик «[{whole}]» — пример: [C10+5:3]", "Unclear counter “[{whole}]” — example: [C10+5:3]");
    let mut rest = spec;
    let mut start = None;
    let mut step = None;
    let mut digits = None;
    if let Some((v, r)) = take_int(rest) {
        start = Some(v);
        rest = r;
    }
    if let Some(r) = rest.strip_prefix('+') {
        let (v, r) = take_int(r).ok_or_else(bad)?;
        step = Some(v);
        rest = r;
    }
    if let Some(r) = rest.strip_prefix(':') {
        let d: usize = r.parse().map_err(|_| bad())?;
        digits = Some(d.min(12));
        rest = "";
    }
    if !rest.is_empty() {
        return Err(bad());
    }
    Ok(Tok::Counter { start, step, digits })
}

fn is_date(spec: &str) -> bool {
    spec.chars().any(|c| "YMDhms".contains(c)) && spec.chars().all(|c| "YMDhms-_. ".contains(c))
}

fn parse_mask(mask: &str) -> Result<Vec<Tok>, String> {
    let mut toks = Vec::new();
    let mut lit = String::new();
    let mut rest = mask;
    while let Some(i) = rest.find(['[', ']']) {
        lit.push_str(&rest[..i]);
        if rest[i..].starts_with(']') {
            return Err(tr!("Лишняя «]» — для самой скобки используйте [[] и []]", "Extra “]” — for the bracket itself use [[] and []]").into());
        }
        let after = &rest[i + 1..];
        // literal brackets: «[[]» and «[]]»
        if let Some(r) = after.strip_prefix("[]") {
            lit.push('[');
            rest = r;
            continue;
        }
        if let Some(r) = after.strip_prefix("]]") {
            lit.push(']');
            rest = r;
            continue;
        }
        let close = after.find(']').ok_or(tr!("Не закрыта скобка «[»", "Unclosed bracket “[”"))?;
        let inner = &after[..close];
        if inner.contains('[') {
            return Err(tr!("Не закрыта скобка «[»", "Unclosed bracket “[”").into());
        }
        rest = &after[close + 1..];
        if !lit.is_empty() {
            toks.push(Tok::Lit(std::mem::take(&mut lit)));
        }
        let mut chars = inner.chars();
        let tok = match chars.next() {
            Some('N') => Tok::Name(parse_range(chars.as_str(), inner)?),
            Some('E') => Tok::Ext(parse_range(chars.as_str(), inner)?),
            Some('C') => parse_counter(chars.as_str(), inner)?,
            _ if inner == "P" => Tok::Parent,
            _ if inner == "G" => Tok::Grand,
            _ if inner == "S" => Tok::Size,
            _ if inner == "U" => Tok::Case(CaseMode::Upper),
            _ if inner == "L" => Tok::Case(CaseMode::Lower),
            _ if inner == "F" => Tok::Case(CaseMode::Words),
            _ if inner == "n" => Tok::Case(CaseMode::Keep),
            _ if is_date(inner) => Tok::Date(inner.to_string()),
            _ => return Err(trf!("Неизвестный элемент маски «[{inner}]»", "Unknown mask element “[{inner}]”")),
        };
        toks.push(tok);
    }
    lit.push_str(rest);
    if !lit.is_empty() {
        toks.push(Tok::Lit(lit));
    }
    Ok(toks)
}

/// Appends text in the current case mode; `[F]` needs to know whether the last
/// char written was inside a word.
struct CaseWriter {
    mode: CaseMode,
    out: String,
    word: bool,
}

impl CaseWriter {
    fn push(&mut self, s: &str) {
        for c in s.chars() {
            match self.mode {
                CaseMode::Keep | CaseMode::First => self.out.push(c),
                CaseMode::Lower => self.out.extend(c.to_lowercase()),
                CaseMode::Upper => self.out.extend(c.to_uppercase()),
                CaseMode::Words => {
                    if self.word {
                        self.out.extend(c.to_lowercase());
                    } else {
                        self.out.extend(c.to_uppercase());
                    }
                }
            }
            self.word = c.is_alphanumeric() || c == '\'';
        }
    }
}

fn date_text(spec: &str, t: Option<DateTime<Local>>) -> String {
    let Some(t) = t else { return String::new() };
    spec.chars()
        .map(|c| match c {
            'Y' => format!("{:04}", t.year()),
            'M' => format!("{:02}", t.month()),
            'D' => format!("{:02}", t.day()),
            'h' => format!("{:02}", t.hour()),
            'm' => format!("{:02}", t.minute()),
            's' => format!("{:02}", t.second()),
            o => o.to_string(),
        })
        .collect()
}

fn counter_text(value: i64, digits: usize) -> String {
    if value < 0 {
        format!("-{:0digits$}", -value)
    } else {
        format!("{value:0digits$}")
    }
}

struct Compiled {
    name: Vec<Tok>,
    ext: Vec<Tok>,
    replace: Replacer,
}

enum Replacer {
    None,
    Regex(regex::Regex, String),
    /// (search, replacement) pairs, applied left to right.
    Plain(Vec<(String, String)>, bool),
}

fn compile(rules: &Rules) -> Result<Compiled, String> {
    let name = parse_mask(&rules.name_mask).map_err(|e| trf!("Маска имени: {e}", "Name mask: {e}"))?;
    let ext = parse_mask(&rules.ext_mask).map_err(|e| trf!("Маска расширения: {e}", "Extension mask: {e}"))?;
    let replace = if rules.find.is_empty() {
        Replacer::None
    } else if rules.regex {
        let re = RegexBuilder::new(&rules.find)
            .case_insensitive(!rules.match_case)
            .build()
            .map_err(|e| trf!("Регулярное выражение: {}", "Regular expression: {}", e.to_string().lines().last().unwrap_or_default().trim()))?;
        Replacer::Regex(re, rules.replace.clone())
    } else {
        let finds: Vec<&str> = rules.find.split('|').collect();
        let reps: Vec<&str> = rules.replace.split('|').collect();
        let pairs = finds
            .iter()
            .enumerate()
            .filter(|(_, f)| !f.is_empty())
            // fewer replacements than alternatives: the last one (or nothing) is reused
            .map(|(i, f)| (f.to_string(), reps.get(i).or(reps.last()).unwrap_or(&"").to_string()))
            .collect();
        Replacer::Plain(pairs, rules.match_case)
    };
    Ok(Compiled { name, ext, replace })
}

fn replace_plain(s: &str, find: &str, rep: &str, match_case: bool) -> String {
    if match_case {
        return s.replace(find, rep);
    }
    // case-insensitive, char-wise so that non-ASCII names keep their byte offsets
    let hay: Vec<char> = s.chars().collect();
    let needle: Vec<char> = find.chars().flat_map(char::to_lowercase).collect();
    let low: Vec<char> = hay.iter().map(|c| c.to_lowercase().next().unwrap_or(*c)).collect();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < hay.len() {
        if i + needle.len() <= hay.len() && low[i..i + needle.len()] == needle[..] {
            out.push_str(rep);
            i += needle.len();
        } else {
            out.push(hay[i]);
            i += 1;
        }
    }
    out
}

fn expand(toks: &[Tok], src: &Source, stem: &str, ext: &str, index: usize, rules: &Rules) -> String {
    let mut w = CaseWriter { mode: CaseMode::Keep, out: String::new(), word: false };
    let date = src.modified.map(DateTime::<Local>::from);
    for t in toks {
        match t {
            Tok::Lit(s) => w.push(s),
            Tok::Name(r) => w.push(&r.cut(stem)),
            Tok::Ext(r) => w.push(&r.cut(ext)),
            Tok::Parent => w.push(&src.ancestor_name(1)),
            Tok::Grand => w.push(&src.ancestor_name(2)),
            Tok::Counter { start, step, digits } => {
                let v = start.unwrap_or(rules.start) + step.unwrap_or(rules.step) * index as i64;
                w.push(&counter_text(v, digits.unwrap_or(rules.digits)));
            }
            Tok::Date(spec) => w.push(&date_text(spec, date)),
            Tok::Size => w.push(&src.size.to_string()),
            Tok::Case(m) => w.mode = *m,
        }
    }
    w.out
}

/// Check a single name against the Windows naming rules.
pub fn validate_name(name: &str) -> Option<Problem> {
    if name.is_empty() || name == "." || name == ".." {
        return Some(Problem::Empty);
    }
    if name.chars().any(|c| matches!(c, '\\' | '/' | ':' | '*' | '?' | '"' | '<' | '>' | '|') || (c as u32) < 32) {
        return Some(Problem::BadChars);
    }
    if name.ends_with(['.', ' ']) {
        return Some(Problem::TrailingDotOrSpace);
    }
    if name.chars().count() > 255 {
        return Some(Problem::TooLong);
    }
    let stem = name.split('.').next().unwrap_or("").trim_end().to_ascii_uppercase();
    let reserved = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (stem.len() == 4
            && (stem.starts_with("COM") || stem.starts_with("LPT"))
            && stem.as_bytes()[3].is_ascii_digit()
            && stem.as_bytes()[3] != b'0');
    reserved.then_some(Problem::Reserved)
}

fn key(p: &Path) -> String {
    p.to_string_lossy().to_lowercase()
}

/// New names for `items` (in this order — the counter follows it). `include[i] == false`
/// keeps item `i` as it is. `exists` tells whether a path is taken on disk.
/// Errors are about the rules themselves (bad mask or regex).
pub fn plan(items: &[Source], include: &[bool], rules: &Rules, exists: impl Fn(&Path) -> bool) -> Result<Vec<Planned>, String> {
    let c = compile(rules)?;
    let mut out = Vec::with_capacity(items.len());
    let mut index = 0;
    for (i, src) in items.iter().enumerate() {
        let old = src.name();
        if !include.get(i).copied().unwrap_or(true) {
            out.push(Planned { new_name: old, changed: false, problem: None });
            continue;
        }
        let (stem, ext) = split_name(&old, src.is_dir);
        let n = expand(&c.name, src, stem, ext, index, rules);
        let e = expand(&c.ext, src, stem, ext, index, rules);
        index += 1;
        let mut name = if e.is_empty() { n } else { format!("{n}.{e}") };
        match &c.replace {
            Replacer::None => {}
            Replacer::Regex(re, rep) => name = re.replace_all(&name, rep.as_str()).into_owned(),
            Replacer::Plain(pairs, mc) => {
                for (f, r) in pairs {
                    name = replace_plain(&name, f, r, *mc);
                }
            }
        }
        name = rules.case.apply(&name);
        out.push(Planned { changed: name != old, problem: validate_name(&name), new_name: name });
    }

    // conflicts: inside the batch, and with what stays on disk
    let sources: HashMap<String, usize> = items.iter().enumerate().map(|(i, s)| (key(&s.path), i)).collect();
    let mut targets: HashMap<String, Vec<usize>> = HashMap::new();
    for (i, (src, p)) in items.iter().zip(&out).enumerate() {
        let target = src.path.with_file_name(&p.new_name);
        targets.entry(key(&target)).or_default().push(i);
    }
    for (k, ids) in &targets {
        if ids.len() > 1 {
            // an item that keeps its name owns it; the others are told it's taken
            let kept = ids.iter().any(|&i| !out[i].changed);
            for &i in ids {
                if out[i].problem.is_none() && out[i].changed {
                    out[i].problem = Some(if kept { Problem::Exists } else { Problem::Duplicate });
                }
            }
            continue;
        }
        let i = ids[0];
        if !out[i].changed || out[i].problem.is_some() {
            continue;
        }
        // the target is another item of the batch: it moves away (unless it stays as it is)
        let freed = sources.get(k).is_some_and(|&j| j != i && out[j].changed);
        let own = *k == key(&items[i].path); // case-only change
        if !freed && !own && exists(&items[i].path.with_file_name(&out[i].new_name)) {
            out[i].problem = Some(Problem::Exists);
        }
        if sources.get(k).is_some_and(|&j| j != i && !out[j].changed) {
            out[i].problem = Some(Problem::Exists);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn src(name: &str) -> Source {
        Source { path: PathBuf::from("C:/Photos/Trip").join(name), is_dir: false, size: 1234, modified: None }
    }

    fn names(items: &[Source], rules: &Rules) -> Vec<String> {
        plan(items, &[], rules, |_| false).unwrap().into_iter().map(|p| p.new_name).collect()
    }

    fn one(name: &str, mask: &str) -> String {
        let rules = Rules { name_mask: mask.into(), ..Rules::default() };
        names(&[src(name)], &rules).remove(0)
    }

    #[test]
    fn default_rules_keep_names() {
        let items = [src("a.txt"), src("Makefile"), src(".gitignore")];
        let p = plan(&items, &[], &Rules::default(), |_| true).unwrap();
        assert_eq!(p.iter().map(|p| p.new_name.as_str()).collect::<Vec<_>>(), ["a.txt", "Makefile", ".gitignore"]);
        assert!(p.iter().all(|p| !p.changed && p.problem.is_none()));
    }

    #[test]
    fn name_ranges() {
        assert_eq!(one("abcdefgh.txt", "[N2-4]"), "bcd.txt");
        assert_eq!(one("abcdefgh.txt", "[N2,3]"), "bcd.txt");
        assert_eq!(one("abcdefgh.txt", "[N3]"), "c.txt");
        assert_eq!(one("abcdefgh.txt", "[N6-]"), "fgh.txt");
        assert_eq!(one("abcdefgh.txt", "[N-3-]"), "fgh.txt");
        assert_eq!(one("abcdefgh.txt", "[N2--2]"), "bcdefg.txt");
        assert_eq!(one("abc.txt", "[N2-40]"), "bc.txt");
        assert_eq!(one("привет.txt", "[N1-3]"), "при.txt");
    }

    #[test]
    fn counter_parent_and_literals() {
        let items = [src("x.jpg"), src("y.jpg"), src("z.jpg")];
        let rules = Rules { name_mask: "[P]_[C]".into(), digits: 3, start: 9, ..Rules::default() };
        assert_eq!(names(&items, &rules), ["Trip_009.jpg", "Trip_010.jpg", "Trip_011.jpg"]);
        let rules = Rules { name_mask: "[[][C10+5:2][]]".into(), ..Rules::default() };
        assert_eq!(names(&items, &rules), ["[10].jpg", "[15].jpg", "[20].jpg"]);
    }

    #[test]
    fn excluded_items_skip_the_counter() {
        let items = [src("a.jpg"), src("b.jpg"), src("c.jpg")];
        let rules = Rules { name_mask: "[C]".into(), ..Rules::default() };
        let p = plan(&items, &[true, false, true], &rules, |_| false).unwrap();
        assert_eq!(p.iter().map(|p| p.new_name.as_str()).collect::<Vec<_>>(), ["1.jpg", "b.jpg", "2.jpg"]);
    }

    #[test]
    fn case_tokens_and_case_mode() {
        assert_eq!(one("hello world.txt", "[U][N]"), "HELLO WORLD.txt");
        assert_eq!(one("hello world.txt", "[F][N]"), "Hello World.txt");
        assert_eq!(one("hello world.txt", "[U][N1][n][N2-]"), "Hello world.txt");
        let rules = Rules { case: CaseMode::Lower, ..Rules::default() };
        assert_eq!(names(&[src("IMG_01.JPG")], &rules), ["img_01.jpg"]);
        let rules = Rules { case: CaseMode::First, ..Rules::default() };
        assert_eq!(names(&[src("мОЙ фАЙЛ.TXT")], &rules), ["Мой файл.txt"]);
    }

    #[test]
    fn extension_mask() {
        let rules = Rules { ext_mask: "[U][E]".into(), ..Rules::default() };
        assert_eq!(names(&[src("a.jpg")], &rules), ["a.JPG"]);
        let rules = Rules { ext_mask: String::new(), ..Rules::default() };
        assert_eq!(names(&[src("a.jpg")], &rules), ["a"]);
    }

    #[test]
    fn date_tokens() {
        let mut s = src("a.txt");
        s.modified = Some(SystemTime::from(DateTime::parse_from_rfc3339("2024-03-07T09:05:02+00:00").unwrap()));
        let local = DateTime::<Local>::from(s.modified.unwrap());
        let rules = Rules { name_mask: "[Y-M-D]_[N]".into(), ..Rules::default() };
        assert_eq!(names(&[s], &rules), [format!("{}_a.txt", local.format("%Y-%m-%d"))]);
    }

    #[test]
    fn search_and_replace() {
        let rules = Rules { find: "IMG_|dsc".into(), replace: "Фото_|Кадр_".into(), ..Rules::default() };
        assert_eq!(names(&[src("img_1.jpg"), src("DSC2.jpg")], &rules), ["Фото_1.jpg", "Кадр_2.jpg"]);
        let rules = Rules { find: r"(\d+)-(\d+)".into(), replace: "$2-$1".into(), regex: true, ..Rules::default() };
        assert_eq!(names(&[src("10-20.txt")], &rules), ["20-10.txt"]);
        let rules = Rules { find: "A".into(), replace: "b".into(), match_case: true, ..Rules::default() };
        assert_eq!(names(&[src("aA.txt")], &rules), ["ab.txt"]);
    }

    #[test]
    fn mask_errors() {
        for bad in ["[X]", "[N", "a]", "[N0]", "[N2-x]", "[Cq]"] {
            let rules = Rules { name_mask: bad.into(), ..Rules::default() };
            assert!(plan(&[src("a.txt")], &[], &rules, |_| false).is_err(), "{bad}");
        }
        let rules = Rules { find: "(".into(), regex: true, ..Rules::default() };
        assert!(plan(&[src("a.txt")], &[], &rules, |_| false).is_err());
    }

    #[test]
    fn problems() {
        assert_eq!(validate_name("a:b"), Some(Problem::BadChars));
        assert_eq!(validate_name("con.txt"), Some(Problem::Reserved));
        assert_eq!(validate_name("COM1"), Some(Problem::Reserved));
        assert_eq!(validate_name("COM0"), None);
        assert_eq!(validate_name("a."), Some(Problem::TrailingDotOrSpace));
        assert_eq!(validate_name(""), Some(Problem::Empty));
        assert_eq!(validate_name("ok.txt"), None);
    }

    #[test]
    fn conflicts() {
        let items = [src("a.txt"), src("b.txt")];
        // both become «x.txt»
        let rules = Rules { name_mask: "x".into(), ..Rules::default() };
        let p = plan(&items, &[], &rules, |_| false).unwrap();
        assert!(p.iter().all(|p| p.problem == Some(Problem::Duplicate)));
        // taken on disk
        let rules = Rules { name_mask: "[N]2".into(), ..Rules::default() };
        let p = plan(&items[..1], &[], &rules, |p| p.ends_with("a2.txt")).unwrap();
        assert_eq!(p[0].problem, Some(Problem::Exists));
        // swap a <-> b is fine: each target is renamed away
        let rules = Rules { find: "a|b|#".into(), replace: "#|a|b".into(), ..Rules::default() };
        let p = plan(&items, &[], &rules, |_| true).unwrap();
        assert_eq!(p.iter().map(|p| p.new_name.as_str()).collect::<Vec<_>>(), ["b.txt", "a.txt"]);
        assert!(p.iter().all(|p| p.problem.is_none()));
        // target is an item that stays (excluded)
        let rules = Rules { name_mask: "b".into(), ..Rules::default() };
        let p = plan(&items, &[true, false], &rules, |_| true).unwrap();
        assert_eq!(p[0].problem, Some(Problem::Exists));
        // case-only change is allowed even though the file "exists"
        let rules = Rules { case: CaseMode::Upper, ..Rules::default() };
        let p = plan(&items[..1], &[], &rules, |_| true).unwrap();
        assert_eq!((p[0].new_name.as_str(), p[0].problem), ("A.TXT", None));
    }
}
