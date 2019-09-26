use crate::Username;
use base64;
use std::str::FromStr;

// Helpers for parsing authorization methods
pub const BEARER_TOKEN_START: usize = 7;
pub const BASIC_TOKEN_START: usize = 6;

#[derive(Eq, PartialEq, Debug, Clone)]
pub struct Auth {
    username: Username,
    password: String,
}

impl FromStr for Auth {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let s = s.replace("%3A", ":"); // When the token is received over the wire, it is URL encoded. We must decode the semi-colons.
        if s.starts_with("Basic") {
            Auth::parse_basic(&s[BASIC_TOKEN_START..])
        } else if s.starts_with("Bearer") {
            Auth::parse_bearer(&s[BEARER_TOKEN_START..])
        } else {
            Auth::parse_text(&s)
        }
    }
}

type AuthResult = Result<Auth, String>;

impl Auth {
    pub fn new(username: &str, password: &str) -> AuthResult {
        Ok(Auth {
            username: Username::from_str(username)?,
            password: password.to_owned(),
        })
    }

    pub fn to_bearer(&self) -> String {
        format!("Bearer {}:{}", self.username, self.password)
    }

    pub fn username(&self) -> &Username {
        &self.username
    }

    pub fn password(&self) -> &str {
        &self.password
    }

    fn parse_basic(s: &str) -> AuthResult {
        let decoded =
            base64::decode(&s).map_err(|_| "could not decode token from base64".to_owned())?;
        let text = std::str::from_utf8(&decoded)
            .map_err(|_| "could not decode token from utf8".to_owned())?;
        Auth::parse_text(text)
    }

    // Currently, we use bearer tokens in a non-standard way, where they each have a
    // username and a password in them. In the future, we will deprecate
    // the non-standard Bearer auth for accounts.
    fn parse_bearer(s: &str) -> AuthResult {
        Auth::parse_text(s)
    }

    fn parse_text(text: &str) -> AuthResult {
        let parts = &mut text.split(':');
        let username = match parts.next() {
            Some(part) => Username::from_str(part)?,
            None => return Err("no username found when parsing auth token".to_owned()),
        };
        let password = match parts.next() {
            Some(part) => part.to_owned(),
            None => return Err("no password found when parsing auth token".to_owned()),
        };
        Ok(Auth { username, password })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fails_correctly() {
        assert_eq!(
            Auth::from_str("Bearer a").unwrap_err(),
            "invalid username format",
        );
        assert_eq!(
            Auth::from_str("Bearer interledger").unwrap_err(),
            "no password found when parsing auth token",
        );
        assert_eq!(
            Auth::from_str("Basic aDFFDJFDJHFDJHF6cnVzdA==").unwrap_err(),
            "could not decode token from utf8"
        );
        assert_eq!(
            Auth::from_str("Basic 0xasdffsaasdf").unwrap_err(),
            "could not decode token from base64"
        );
    }

    #[test]
    fn parses_correctly() {
        assert_eq!(
            Auth::from_str("Basic aW50ZXJsZWRnZXI6cnVzdA==").unwrap(),
            Auth::new("interledger", "rust").unwrap()
        );

        assert_eq!(
            Auth::from_str("Bearer interledger%3Arust").unwrap(),
            Auth::new("interledger", "rust").unwrap()
        );

        assert_eq!(
            Auth::from_str("Bearer interledger:rust").unwrap(),
            Auth::new("interledger", "rust").unwrap()
        );

        assert!(Auth::from_str("SomethingElse asdf").is_err());
        assert!(Auth::from_str("Basic asdf").is_err());
        assert!(Auth::from_str("Bearer asdf").is_err());

        assert_eq!(
            Auth::new("interledger", "rust").unwrap().to_bearer(),
            "Bearer interledger:rust"
        );
    }
}
