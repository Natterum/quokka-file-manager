//! Filename masks for selection. Several masks are separated with semicolons.
use crate::tr;

pub fn masks(raw: &str) -> Result<regex::Regex, String> {
    let mut patterns = Vec::new();
    for mask in raw.split(';').map(str::trim).filter(|s| !s.is_empty()) {
        let mask = if mask == "*.*" { "*" } else { mask };
        let mut pattern = String::new();
        for ch in mask.chars() {
            match ch {
                '*' => pattern.push_str(".*"),
                '?' => pattern.push('.'),
                c => pattern.push_str(&regex::escape(&c.to_string())),
            }
        }
        patterns.push(pattern);
    }
    if patterns.is_empty() {
        return Err(tr!(
            "Введите маску, например *.jpg;*.png",
            "Enter a mask, e.g. *.jpg;*.png"
        )
        .into());
    }
    regex::RegexBuilder::new(&format!("^(?:{})$", patterns.join("|")))
        .case_insensitive(true)
        .size_limit(1 << 20)
        .build()
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn filename_masks_are_anchored_literal_and_unicode_aware() {
        let re = masks(" *.jpg ; фото?.png ; a[1].txt ").unwrap();
        for name in ["IMG.JPG", "фотоЯ.png", "a[1].txt"] {
            assert!(re.is_match(name));
        }
        for name in ["a.jpg.bak", "фото.png", "a1.txt"] {
            assert!(!re.is_match(name));
        }
        assert!(masks("*.*").unwrap().is_match("README"));
        assert!(masks(" ; ").is_err());
    }
}
