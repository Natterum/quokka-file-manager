//! Update rules: version comparison and the names of the release assets that CI
//! publishes on GitHub Releases. No I/O — the infra layer downloads.

use std::cmp::Ordering;

/// `MAJOR.MINOR.PATCH[-pre]`, compared by SemVer precedence (build metadata ignored).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Version {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
    /// Pre-release identifiers (`rc.1` → `["rc", "1"]`); empty for a release.
    pub pre: Vec<String>,
}

impl Version {
    /// Accepts an optional leading `v` (`v0.2.0` — the tag form).
    pub fn parse(s: &str) -> Option<Version> {
        let s = s.trim();
        let s = s.strip_prefix(['v', 'V']).unwrap_or(s);
        let s = s.split_once('+').map_or(s, |(v, _)| v);
        let (core, pre) = match s.split_once('-') {
            Some((c, p)) => (c, p.split('.').map(str::to_string).collect::<Vec<_>>()),
            None => (s, Vec::new()),
        };
        if pre.iter().any(|p| p.is_empty()) {
            return None;
        }
        let mut nums = core.split('.').map(|n| if n.is_empty() || !n.bytes().all(|b| b.is_ascii_digit()) { None } else { n.parse::<u64>().ok() });
        let v = Version { major: nums.next()??, minor: nums.next()??, patch: nums.next()??, pre };
        if nums.next().is_some() {
            return None;
        }
        Some(v)
    }

    pub fn is_prerelease(&self) -> bool {
        !self.pre.is_empty()
    }

    /// `MAJOR.MINOR.PATCH` — the only form an MSI `ProductVersion` accepts.
    pub fn numeric(&self) -> String {
        format!("{}.{}.{}", self.major, self.minor, self.patch)
    }
}

impl std::fmt::Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)?;
        if !self.pre.is_empty() {
            write!(f, "-{}", self.pre.join("."))?;
        }
        Ok(())
    }
}

fn cmp_ident(a: &str, b: &str) -> Ordering {
    match (a.parse::<u64>(), b.parse::<u64>()) {
        (Ok(x), Ok(y)) => x.cmp(&y),
        (Ok(_), Err(_)) => Ordering::Less,
        (Err(_), Ok(_)) => Ordering::Greater,
        (Err(_), Err(_)) => a.cmp(b),
    }
}

impl Ord for Version {
    fn cmp(&self, o: &Self) -> Ordering {
        (self.major, self.minor, self.patch).cmp(&(o.major, o.minor, o.patch)).then_with(|| {
            match (self.pre.is_empty(), o.pre.is_empty()) {
                (true, true) => Ordering::Equal,
                (true, false) => Ordering::Greater,
                (false, true) => Ordering::Less,
                (false, false) => {
                    for (a, b) in self.pre.iter().zip(&o.pre) {
                        let c = cmp_ident(a, b);
                        if c != Ordering::Equal {
                            return c;
                        }
                    }
                    self.pre.len().cmp(&o.pre.len())
                }
            }
        })
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, o: &Self) -> Option<Ordering> {
        Some(self.cmp(o))
    }
}

/// True when `latest` should be offered to a user running `current`.
pub fn is_newer(latest: &str, current: &str) -> bool {
    match (Version::parse(latest), Version::parse(current)) {
        (Some(l), Some(c)) => l > c,
        _ => false,
    }
}

/// How this copy of Quokka was installed, which decides how it updates itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InstallKind {
    /// Installed by the MSI package: the new MSI upgrades it in place.
    Msi,
    /// Unpacked from the ZIP package: the executable is swapped for the new one.
    Portable,
}

/// Release asset names produced by `.github/scripts/build-windows.ps1`.
pub fn zip_asset(version: &Version, arch: &str) -> String {
    format!("quokka-v{version}-windows-{arch}.zip")
}

pub fn msi_asset(version: &Version, arch: &str) -> String {
    format!("quokka-v{version}-windows-{arch}.msi")
}

/// The asset to download for an install of this kind.
pub fn asset_for(kind: InstallKind, version: &Version, arch: &str) -> String {
    match kind {
        InstallKind::Msi => msi_asset(version, arch),
        InstallKind::Portable => zip_asset(version, arch),
    }
}

/// The checksum file published next to every asset.
pub fn checksum_asset(asset: &str) -> String {
    format!("{asset}.sha256")
}

/// Hash from a `sha256sum`-style line (`<hex>  <name>`), lowercased.
pub fn parse_checksum(text: &str) -> Option<String> {
    let hex = text.split_whitespace().next()?.to_ascii_lowercase();
    (hex.len() == 64 && hex.bytes().all(|b| b.is_ascii_hexdigit())).then_some(hex)
}

/// Seconds between automatic checks.
pub const CHECK_INTERVAL_SECS: i64 = 24 * 60 * 60;

/// Whether an automatic check is due (the clock may also have gone backwards).
pub fn check_due(last: i64, now: i64) -> bool {
    last <= 0 || now < last || now - last >= CHECK_INTERVAL_SECS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_tags_and_versions() {
        let v = Version::parse("v1.2.3").unwrap();
        assert_eq!((v.major, v.minor, v.patch), (1, 2, 3));
        assert!(!v.is_prerelease());
        let v = Version::parse("0.2.0-rc.1+build.5").unwrap();
        assert_eq!(v.pre, ["rc", "1"]);
        assert_eq!(v.to_string(), "0.2.0-rc.1");
        assert_eq!(v.numeric(), "0.2.0");
        for bad in ["", "1.2", "1.2.3.4", "1..3", "a.b.c", "1.2.3-", "1.2.3-rc..1", "1.2.-3"] {
            assert!(Version::parse(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn orders_by_semver_precedence() {
        let ordered = ["0.1.0", "0.1.1", "0.2.0-alpha", "0.2.0-alpha.1", "0.2.0-alpha.beta", "0.2.0-beta.2", "0.2.0-beta.11", "0.2.0-rc.1", "0.2.0", "1.0.0", "1.10.0"];
        for w in ordered.windows(2) {
            assert!(is_newer(w[1], w[0]), "{} > {}", w[1], w[0]);
            assert!(!is_newer(w[0], w[1]), "{} < {}", w[0], w[1]);
        }
        assert!(!is_newer("v0.1.0", "0.1.0"));
        assert!(!is_newer("garbage", "0.1.0"));
    }

    #[test]
    fn asset_names_match_the_ci_packages() {
        let v = Version::parse("v0.3.0").unwrap();
        assert_eq!(asset_for(InstallKind::Portable, &v, "x64"), "quokka-v0.3.0-windows-x64.zip");
        assert_eq!(asset_for(InstallKind::Msi, &v, "x86"), "quokka-v0.3.0-windows-x86.msi");
        assert_eq!(checksum_asset("a.msi"), "a.msi.sha256");
    }

    #[test]
    fn reads_checksum_lines() {
        let h = "A".repeat(64);
        assert_eq!(parse_checksum(&format!("{h}  quokka.zip\r\n")), Some("a".repeat(64)));
        assert_eq!(parse_checksum("abc  x"), None);
        assert_eq!(parse_checksum(""), None);
    }

    #[test]
    fn automatic_check_runs_once_a_day() {
        assert!(check_due(0, 1_000));
        assert!(!check_due(1_000, 1_000 + CHECK_INTERVAL_SECS - 1));
        assert!(check_due(1_000, 1_000 + CHECK_INTERVAL_SECS));
        assert!(check_due(5_000, 1_000));
    }
}
