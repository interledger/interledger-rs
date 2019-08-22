/// Wrapper around String to perform sanitization for usernames:
/// 1. Normalizes string input using NFKC unicode normalization (using this function)
/// 2. Validates that the string matches the regex /^\w{1,32}$/u (the unicode flag is set)
/// 3. Uses case-folding to convert to lowercase in a language-sensitive way (possibly using unicase)
/// 4. (Possibly) Check that all characters are part of the same script or disallow "confusable" characters (using unicode-script or unicode_skeleton)
///
///
use regex::Regex;
use std::fmt::Display;
use std::str::FromStr;

use lazy_static::lazy_static;
use serde::{Deserialize, Serialize};

lazy_static! {
    static ref USERNAME_PATTERN: Regex = Regex::new(r"/^\w{1,32}$/u").unwrap();
}

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
        Ok(Username(src.to_owned()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_correctly() {
        assert_eq!(
            Username::from_str("alice").unwrap(),
            Username("alice".to_owned())
        );
    }

}
