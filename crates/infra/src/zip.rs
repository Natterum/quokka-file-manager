//! Minimal streaming ZIP writer (deflate, UTF-8 names). Header sizes are patched
//! in place after each entry is compressed, so files never sit fully in memory.

use quokka_domain::{tr, trf};
use std::fs::File;
use std::io::{self, BufWriter, Read, Seek, SeekFrom, Write};
use std::time::SystemTime;

struct CentralEntry {
    name: Vec<u8>,
    crc: u32,
    csize: u32,
    usize: u32,
    offset: u32,
    time: u16,
    date: u16,
    dir: bool,
    method: u16,
}

pub struct ZipWriter {
    out: BufWriter<File>,
    entries: Vec<CentralEntry>,
}

const FLAG_UTF8: u16 = 1 << 11;
const LIMIT: u64 = u32::MAX as u64;

fn too_big() -> io::Error {
    io::Error::new(io::ErrorKind::Other, tr!("архив больше 4 ГБ не поддерживается", "archives larger than 4 GB are not supported"))
}

fn dos_time(t: Option<SystemTime>) -> (u16, u16) {
    use chrono::{Datelike, Timelike};
    let dt = chrono::DateTime::<chrono::Local>::from(t.unwrap_or_else(SystemTime::now));
    let year = (dt.year().clamp(1980, 2107) - 1980) as u16;
    let date = (year << 9) | ((dt.month() as u16) << 5) | dt.day() as u16;
    let time = ((dt.hour() as u16) << 11) | ((dt.minute() as u16) << 5) | (dt.second() as u16 / 2);
    (time, date)
}

impl ZipWriter {
    pub fn create(path: &std::path::Path) -> io::Result<Self> {
        Ok(ZipWriter { out: BufWriter::new(File::create(path)?), entries: Vec::new() })
    }

    fn pos(&mut self) -> io::Result<u64> {
        self.out.stream_position()
    }

    fn local_header(&mut self, e: &CentralEntry) -> io::Result<()> {
        let w = &mut self.out;
        w.write_all(&0x0403_4b50u32.to_le_bytes())?;
        w.write_all(&20u16.to_le_bytes())?; // version needed
        w.write_all(&FLAG_UTF8.to_le_bytes())?;
        w.write_all(&e.method.to_le_bytes())?;
        w.write_all(&e.time.to_le_bytes())?;
        w.write_all(&e.date.to_le_bytes())?;
        w.write_all(&e.crc.to_le_bytes())?;
        w.write_all(&e.csize.to_le_bytes())?;
        w.write_all(&e.usize.to_le_bytes())?;
        w.write_all(&(e.name.len() as u16).to_le_bytes())?;
        w.write_all(&0u16.to_le_bytes())?;
        w.write_all(&e.name)
    }

    pub fn add_dir(&mut self, name: &str, modified: Option<SystemTime>) -> io::Result<()> {
        let offset = self.pos()?;
        if offset > LIMIT {
            return Err(too_big());
        }
        let (time, date) = dos_time(modified);
        let mut n = name.replace('\\', "/");
        if !n.ends_with('/') {
            n.push('/');
        }
        let e = CentralEntry {
            name: n.into_bytes(),
            crc: 0,
            csize: 0,
            usize: 0,
            offset: offset as u32,
            time,
            date,
            dir: true,
            method: 0,
        };
        self.local_header(&e)?;
        self.entries.push(e);
        Ok(())
    }

    /// Compress `src` into the archive; `progress` gets the number of bytes read so far
    /// and may return an error to abort (e.g. on cancellation).
    pub fn add_file(
        &mut self,
        name: &str,
        src: &mut impl Read,
        modified: Option<SystemTime>,
        mut progress: impl FnMut(u64) -> io::Result<()>,
    ) -> io::Result<()> {
        let offset = self.pos()?;
        if offset > LIMIT {
            return Err(too_big());
        }
        let (time, date) = dos_time(modified);
        let mut e = CentralEntry {
            name: name.replace('\\', "/").into_bytes(),
            crc: 0,
            csize: 0,
            usize: 0,
            offset: offset as u32,
            time,
            date,
            dir: false,
            method: 8,
        };
        self.local_header(&e)?;
        let data_start = self.pos()?;

        let mut hasher = crc32fast::Hasher::new();
        let mut total: u64 = 0;
        {
            let mut enc = flate2::write::DeflateEncoder::new(&mut self.out, flate2::Compression::default());
            let mut buf = vec![0u8; 256 * 1024];
            loop {
                let n = src.read(&mut buf)?;
                if n == 0 {
                    break;
                }
                hasher.update(&buf[..n]);
                enc.write_all(&buf[..n])?;
                total += n as u64;
                if total > LIMIT {
                    return Err(too_big());
                }
                progress(n as u64)?;
            }
            enc.finish()?;
        }
        let end = self.pos()?;
        if end > LIMIT {
            return Err(too_big());
        }
        e.crc = hasher.finalize();
        e.csize = (end - data_start) as u32;
        e.usize = total as u32;
        // patch crc and sizes in the local header (offset + 14)
        self.out.seek(SeekFrom::Start(offset + 14))?;
        self.out.write_all(&e.crc.to_le_bytes())?;
        self.out.write_all(&e.csize.to_le_bytes())?;
        self.out.write_all(&e.usize.to_le_bytes())?;
        self.out.seek(SeekFrom::Start(end))?;
        self.entries.push(e);
        Ok(())
    }

    pub fn finish(mut self) -> io::Result<()> {
        if self.entries.len() > u16::MAX as usize {
            return Err(io::Error::new(io::ErrorKind::Other, tr!("слишком много файлов для ZIP (больше 65535)", "too many files for ZIP (more than 65535)")));
        }
        let cd_start = self.pos()?;
        let entries = std::mem::take(&mut self.entries);
        for e in &entries {
            let w = &mut self.out;
            w.write_all(&0x0201_4b50u32.to_le_bytes())?;
            w.write_all(&20u16.to_le_bytes())?; // version made by (MS-DOS)
            w.write_all(&20u16.to_le_bytes())?;
            w.write_all(&FLAG_UTF8.to_le_bytes())?;
            w.write_all(&e.method.to_le_bytes())?;
            w.write_all(&e.time.to_le_bytes())?;
            w.write_all(&e.date.to_le_bytes())?;
            w.write_all(&e.crc.to_le_bytes())?;
            w.write_all(&e.csize.to_le_bytes())?;
            w.write_all(&e.usize.to_le_bytes())?;
            w.write_all(&(e.name.len() as u16).to_le_bytes())?;
            w.write_all(&0u16.to_le_bytes())?; // extra
            w.write_all(&0u16.to_le_bytes())?; // comment
            w.write_all(&0u16.to_le_bytes())?; // disk
            w.write_all(&0u16.to_le_bytes())?; // internal attrs
            w.write_all(&(if e.dir { 0x10u32 } else { 0x20u32 }).to_le_bytes())?;
            w.write_all(&e.offset.to_le_bytes())?;
            w.write_all(&e.name)?;
        }
        let cd_end = self.pos()?;
        if cd_end > LIMIT {
            return Err(too_big());
        }
        let n = entries.len() as u16;
        let w = &mut self.out;
        w.write_all(&0x0605_4b50u32.to_le_bytes())?;
        w.write_all(&0u16.to_le_bytes())?;
        w.write_all(&0u16.to_le_bytes())?;
        w.write_all(&n.to_le_bytes())?;
        w.write_all(&n.to_le_bytes())?;
        w.write_all(&((cd_end - cd_start) as u32).to_le_bytes())?;
        w.write_all(&(cd_start as u32).to_le_bytes())?;
        w.write_all(&0u16.to_le_bytes())?;
        w.flush()
    }
}

// ───────────────────────────── reading ─────────────────────────────

/// One entry of the central directory of an existing archive.
pub struct ReadEntry {
    /// Name as stored in the archive (`/`-separated).
    pub name: String,
    pub dir: bool,
    pub size: u64,
    csize: u64,
    crc: u32,
    method: u16,
    encrypted: bool,
    offset: u64,
    time: u16,
    date: u16,
}

impl ReadEntry {
    /// Relative path to extract to, or `None` when the name is unsafe
    /// (absolute, a drive prefix or `..` that would escape the destination).
    pub fn safe_path(&self) -> Option<std::path::PathBuf> {
        if self.name.starts_with(['/', '\\']) { return None; }
        let mut out = std::path::PathBuf::new();
        for part in self.name.split(['/', '\\']) {
            match part {
                "" | "." => continue,
                ".." => return None,
                p if p.contains([':', '\0']) || p.ends_with(['.', ' ']) => return None,
                p if {
                    let base = p.split('.').next().unwrap_or("").to_ascii_uppercase();
                    matches!(base.as_str(), "CON" | "PRN" | "AUX" | "NUL") ||
                        (base.len() == 4 && (base.starts_with("COM") || base.starts_with("LPT")) && matches!(base.as_bytes()[3], b'1'..=b'9'))
                } => return None,
                p => out.push(p),
            }
        }
        if out.as_os_str().is_empty() {
            None
        } else {
            Some(out)
        }
    }

    pub fn modified(&self) -> Option<SystemTime> {
        use chrono::TimeZone;
        let (d, t) = (self.date, self.time);
        let dt = chrono::NaiveDate::from_ymd_opt(1980 + (d >> 9) as i32, ((d >> 5) & 0xF) as u32, (d & 0x1F) as u32)?
            .and_hms_opt((t >> 11) as u32, ((t >> 5) & 0x3F) as u32, ((t & 0x1F) * 2) as u32)?;
        chrono::Local.from_local_datetime(&dt).earliest().map(SystemTime::from)
    }
}

/// Names without the UTF-8 flag: modern tools often still write UTF-8, otherwise
/// Windows archivers use the OEM code page — CP866 on Russian systems.
fn decode_name(raw: &[u8], utf8_flag: bool) -> String {
    if utf8_flag || raw.is_ascii() {
        return String::from_utf8_lossy(raw).into_owned();
    }
    if let Ok(s) = std::str::from_utf8(raw) {
        return s.to_string();
    }
    raw.iter()
        .map(|&b| match b {
            0x00..=0x7F => b as char,
            0x80..=0xAF => char::from_u32(0x0410 + (b - 0x80) as u32).unwrap_or('?'),
            0xE0..=0xEF => char::from_u32(0x0440 + (b - 0xE0) as u32).unwrap_or('?'),
            0xF0 => 'Ё',
            0xF1 => 'ё',
            _ => '_',
        })
        .collect()
}

fn bad(msg: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg.to_string())
}

fn u16_at(b: &[u8], i: usize) -> u16 {
    u16::from_le_bytes([b[i], b[i + 1]])
}

fn u32_at(b: &[u8], i: usize) -> u32 {
    u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]])
}

pub struct ZipReader {
    file: File,
    pub entries: Vec<ReadEntry>,
}

impl ZipReader {
    pub fn open(path: &std::path::Path) -> io::Result<Self> {
        let mut file = File::open(path)?;
        let len = file.metadata()?.len();
        if len < 22 {
            return Err(bad(tr!("это не ZIP-архив", "this is not a ZIP archive")));
        }
        // the end-of-central-directory record sits in the last 22 + 65535 (comment) bytes
        let tail_len = len.min(22 + 65535);
        file.seek(SeekFrom::Start(len - tail_len))?;
        let mut tail = vec![0u8; tail_len as usize];
        file.read_exact(&mut tail)?;
        let eocd = (0..=tail.len() - 22)
            .rev()
            .find(|&i| u32_at(&tail, i) == 0x0605_4b50)
            .ok_or_else(|| bad(tr!("это не ZIP-архив", "this is not a ZIP archive")))?;
        let count = u16_at(&tail, eocd + 10) as usize;
        let cd_size = u32_at(&tail, eocd + 12) as u64;
        let cd_start = u32_at(&tail, eocd + 16) as u64;
        if count == 0xFFFF || cd_start == 0xFFFF_FFFF {
            return Err(bad(tr!("архивы ZIP64 (больше 4 ГБ) пока не поддерживаются", "ZIP64 archives (larger than 4 GB) are not supported yet")));
        }
        if cd_start + cd_size > len {
            return Err(bad(tr!("архив повреждён", "archive is corrupted")));
        }
        file.seek(SeekFrom::Start(cd_start))?;
        let mut cd = vec![0u8; cd_size as usize];
        file.read_exact(&mut cd)?;

        let mut entries = Vec::with_capacity(count);
        let mut i = 0;
        while i + 46 <= cd.len() && u32_at(&cd, i) == 0x0201_4b50 {
            let flags = u16_at(&cd, i + 8);
            let n = u16_at(&cd, i + 28) as usize;
            let x = u16_at(&cd, i + 30) as usize;
            let c = u16_at(&cd, i + 32) as usize;
            if i + 46 + n > cd.len() {
                return Err(bad(tr!("архив повреждён", "archive is corrupted")));
            }
            let name = decode_name(&cd[i + 46..i + 46 + n], flags & FLAG_UTF8 != 0);
            let csize = u32_at(&cd, i + 20) as u64;
            let size = u32_at(&cd, i + 24) as u64;
            let offset = u32_at(&cd, i + 42) as u64;
            if csize == 0xFFFF_FFFF || size == 0xFFFF_FFFF || offset == 0xFFFF_FFFF {
                return Err(bad(tr!("архивы ZIP64 (больше 4 ГБ) пока не поддерживаются", "ZIP64 archives (larger than 4 GB) are not supported yet")));
            }
            let attrs = u32_at(&cd, i + 38);
            entries.push(ReadEntry {
                dir: name.ends_with('/') || name.ends_with('\\') || (attrs & 0x10 != 0 && size == 0),
                name,
                size,
                csize,
                crc: u32_at(&cd, i + 16),
                method: u16_at(&cd, i + 10),
                encrypted: flags & 1 != 0,
                offset,
                time: u16_at(&cd, i + 12),
                date: u16_at(&cd, i + 14),
            });
            i += 46 + n + x + c;
        }
        Ok(ZipReader { file, entries })
    }

    /// Decompress entry `idx` into `out`; `progress` gets the number of bytes written
    /// since the previous call and may return an error to abort.
    pub fn extract(
        &mut self,
        idx: usize,
        out: &mut impl Write,
        mut progress: impl FnMut(u64) -> io::Result<()>,
    ) -> io::Result<()> {
        let e = &self.entries[idx];
        if e.encrypted {
            return Err(bad(tr!("файл зашифрован паролем", "file is password-protected")));
        }
        let mut lh = [0u8; 30];
        self.file.seek(SeekFrom::Start(e.offset))?;
        self.file.read_exact(&mut lh)?;
        if u32_at(&lh, 0) != 0x0403_4b50 {
            return Err(bad(tr!("архив повреждён", "archive is corrupted")));
        }
        let data = e.offset + 30 + u16_at(&lh, 26) as u64 + u16_at(&lh, 28) as u64;
        self.file.seek(SeekFrom::Start(data))?;
        let raw = (&mut self.file).take(e.csize);
        let mut src: Box<dyn Read + '_> = match e.method {
            0 => Box::new(raw),
            8 => Box::new(flate2::read::DeflateDecoder::new(io::BufReader::new(raw))),
            m => return Err(bad(&trf!("неподдерживаемый метод сжатия ({m})", "unsupported compression method ({m})"))),
        };
        let mut hasher = crc32fast::Hasher::new();
        let mut buf = vec![0u8; 256 * 1024];
        loop {
            let n = src.read(&mut buf)?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
            out.write_all(&buf[..n])?;
            progress(n as u64)?;
        }
        if hasher.finalize() != e.crc {
            return Err(bad(tr!("контрольная сумма не совпадает — архив повреждён", "checksum mismatch — archive is corrupted")));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_unsafe_names() {
        let dir = std::env::temp_dir().join("quokka-zip-rt");
        let _ = std::fs::create_dir_all(&dir);
        let out = dir.join("rt.zip");
        let data = "Привет, архив! ".repeat(5000);
        let mut z = ZipWriter::create(&out).unwrap();
        z.add_dir("папка", None).unwrap();
        z.add_file("папка/файл.txt", &mut data.as_bytes(), None, |_| Ok(())).unwrap();
        z.add_file("../evil.txt", &mut &b"x"[..], None, |_| Ok(())).unwrap();
        z.finish().unwrap();

        let mut r = ZipReader::open(&out).unwrap();
        assert_eq!(r.entries.len(), 3);
        assert!(r.entries[0].dir);
        assert_eq!(r.entries[1].name, "папка/файл.txt");
        assert_eq!(r.entries[1].size, data.len() as u64);
        assert!(r.entries[2].safe_path().is_none());
        let mut got = Vec::new();
        r.extract(1, &mut got, |_| Ok(())).unwrap();
        assert_eq!(got, data.as_bytes());
    }

    #[test]
    fn cp866_names() {
        assert_eq!(decode_name(&[0x8F, 0xE0, 0xA8, 0xA2, 0xA5, 0xE2], false), "Привет");
    }

    #[test]
    fn writes_readable_archive() {
        let dir = std::env::temp_dir().join("quokka-zip-test");
        let _ = std::fs::create_dir_all(&dir);
        let out = dir.join("t.zip");
        let mut z = ZipWriter::create(&out).unwrap();
        z.add_dir("папка", None).unwrap();
        let data = "Привет, архив! ".repeat(1000);
        z.add_file("папка/файл.txt", &mut data.as_bytes(), None, |_| Ok(())).unwrap();
        z.add_file("empty.bin", &mut &b""[..], None, |_| Ok(())).unwrap();
        z.finish().unwrap();
        let bytes = std::fs::read(&out).unwrap();
        assert_eq!(&bytes[..4], &[0x50, 0x4b, 0x03, 0x04]);
        assert_eq!(&bytes[bytes.len() - 22..bytes.len() - 18], &[0x50, 0x4b, 0x05, 0x06]);
    }
}
