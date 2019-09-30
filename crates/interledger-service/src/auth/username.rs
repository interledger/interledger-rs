/// Wrapper around String to perform sanitization for usernames
use regex::Regex;
use std::{convert::TryFrom, fmt::Display, str::FromStr};

use lazy_static::lazy_static;
use serde::{Deserialize, Serialize};
use unicase::UniCase;
use unicode_normalization::UnicodeNormalization;

lazy_static! {
    // No need to add unicode flags, already supported by the crate.
    static ref USERNAME_PATTERN: Regex = Regex::new(r"^\w{2,32}$").unwrap();
}

/// Usernames can be unicode and must be between 2 and 32 characters.
///
/// This type applies the following checks/transformations to usernames to ensure uniqueness:
/// 1. [NFKC Normalization](https://en.wikipedia.org/wiki/Unicode_equivalence#Normalization)
/// 2. Checks the string is 2-32 word characters only (no special characters except `_`)
/// 3. Uses [case folding](https://www.w3.org/International/wiki/Case_folding) to convert to lowercase in a language-aware manner
#[derive(Clone, Debug, Serialize, Deserialize, PartialOrd, Ord)]
#[serde(try_from = "&str")]
pub struct Username(String);

impl PartialEq for Username {
    fn eq(&self, other: &Self) -> bool {
        UniCase::new(self) == UniCase::new(other)
    }
}

impl Eq for Username {}

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

impl TryFrom<&str> for Username {
    type Error = String;

    fn try_from(value: &str) -> Result<Username, String> {
        Username::from_str(value)
    }
}

impl FromStr for Username {
    type Err = String;

    fn from_str(src: &str) -> Result<Self, Self::Err> {
        let unicode_normalized = src.nfkc().collect::<String>();
        if !USERNAME_PATTERN.is_match(&unicode_normalized) {
            return Err("invalid username format".to_owned());
        }
        Ok(Username(unicode_normalized))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unicode_normalization_works() {
        assert_eq!(
            Username::from_str("\u{005a}\u{006f}\u{00e9}").unwrap(), // Zoé
            Username::from_str("\u{005a}\u{006f}\u{0065}\u{0301}").unwrap()  //  "Zoé"
        );
    }

    #[test]
    fn case_folding_works() {
        assert_eq!(
            Username::from_str("Coté").unwrap(),
            Username::from_str("coté").unwrap()
        );
        assert_eq!(
            Username::from_str("Maße").unwrap(),
            Username::from_str("MASSE").unwrap()
        );
        assert_eq!(
            Username::from_str("foobar").unwrap(),
            Username::from_str("FoObAr").unwrap()
        );
    }

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

    #[test]
    fn reject_invalid_usernames() {
        let rejected_user = Username::from_str("no-hyphens-allowed");
        assert!(rejected_user.is_err());
    }

    #[test]
    fn deserialize_usernames() {
        use serde_json;
        let rejected_deserialize: Result<Username, _> =
            serde_json::from_str(r#""no-hyphens-allowed""#);
        assert!(rejected_deserialize.is_err());
        let valid_deserialize: Result<Username, _> =
            serde_json::from_str(r#""underscores_fine_though""#);
        assert_eq!(
            valid_deserialize.unwrap().0,
            String::from("underscores_fine_though")
        );
    }
}
