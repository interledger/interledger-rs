/// Wrapper around String to perform sanitization for usernames:
/// 1. Normalizes string input using NFKC unicode normalization (using this function)
/// 2. Validates that the string matches the regex /^\w{1,32}$/u (the unicode flag is set)
/// 3. Uses case-folding to convert to lowercase in a language-sensitive way (possibly using unicase)
use regex::Regex;
use std::fmt::Display;
use std::str::FromStr;

use lazy_static::lazy_static;
use serde::{Deserialize, Serialize};
use unicase::UniCase;
use unicode_normalization::UnicodeNormalization;

lazy_static! {
    // No need to add unicode flags, already supported by the crate.
    static ref USERNAME_PATTERN: Regex = Regex::new(r"^\w{2,32}$").unwrap();
}

/// Usernames can be unicode and must be between 2 and 32 characters
#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq)]
pub struct Username(String);

impl Display for Username {
    fn fmt(&self, f: &mut ::std::fmt::Formatter) -> Result<(), ::std::fmt::Error> {
        f.write_str(&self.0)
    }
}

impl AsRef<str> for Username {
    #[inline]
    fn as_ref(&self) -> &str {
        self.0.as_ref()
    }
}

impl std::ops::Deref for Username {
    type Target = str;

    fn deref(&self) -> &str {
        self.0.as_ref()
    }
}

impl FromStr for Username {
    type Err = ();

    fn from_str(src: &str) -> Result<Self, Self::Err> {
        let unicode_normalized = src.nfkc().collect::<String>();
        if !USERNAME_PATTERN.is_match(&unicode_normalized) {
            return Err(());
        }
        let casefolded = UniCase::new(unicode_normalized);
        Ok(Username(casefolded.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn too_long_name() {
        assert!(Username::from_str("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa").is_err());
    }

    #[test]
    fn too_short_name() {
        assert!(Username::from_str("a").is_err());
    }

    #[test]
    fn mixed_case() {
        assert_eq!(
            Username::from_str("A_lic123").unwrap().to_string(),
            "A_lic123"
        );
    }

    #[test]
    fn numbers_symbols_work() {
        assert_eq!(
            Username::from_str("a_lic123").unwrap().to_string(),
            "a_lic123"
        );
    }

    #[test]
    fn works_with_unicode() {
        assert_eq!(Username::from_str("山本").unwrap().to_string(), "山本");
    }

    #[test]
    fn works() {
        assert_eq!(
            Username::from_str("alice").unwrap(),
            Username("alice".to_owned())
        );
    }

    #[test]
    fn formats_correctly() {
        let user = Username("alice".to_owned());
        assert_eq!(format!("{}:password", user), "alice:password");
    }
}
