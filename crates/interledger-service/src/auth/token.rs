use base64;
use crate::Username;
use std::str::FromStr;

// Helpers for parsing authorization methods
pub const BEARER_TOKEN_START: usize = 7;
pub const BASIC_TOKEN_START: usize = 6;

#[derive(Eq, PartialEq, Debug, Clone)]
pub struct Auth {
    username: Username,
    password: String,
}

impl Auth {
    pub fn new(username: &str, password: &str) -> Result<Self, ()> {
        Ok(Auth {
            username: Username::from_str(username)?,
            password: password.to_owned(),
        })
    }

    pub fn to_bearer(&self) -> String {
        format!("Bearer {}:{}", self.username, self.password)
    }

    pub fn username(&self) -> Username {
        self.username.clone()
    }

    pub fn password(&self) -> &str {
        &self.password
    }

    pub fn parse(s: &str) -> Result<Self, ()> {
        let s = s.replace("%3A", ":"); // When the token is received over the wire, it is URL encoded. We must decode the semi-colons.
        if s.starts_with("Basic") {
            Auth::parse_basic(&s[BASIC_TOKEN_START..])
        } else if s.starts_with("Bearer") {
            Auth::parse_bearer(&s[BEARER_TOKEN_START..])
        } else {
            Auth::parse_text(&s)
        }
    }

    fn parse_basic(s: &str) -> Result<Self, ()> {
        let decoded = base64::decode(&s).map_err(|_| ())?;
        let text = std::str::from_utf8(&decoded).map_err(|_| ())?;
        Auth::parse_text(text)
    }

    // Currently, we use bearer tokens in a non-standard way, where they each have a
    // username and a password in them. In the future, we will deprecate Bearer auth
    // for accounts.
    fn parse_bearer(s: &str) -> Result<Auth, ()> {
        Auth::parse_text(s)
    }

    fn parse_text(text: &str) -> Result<Self, ()> {
        let parts = &mut text.split(':');
        let username = match parts.next() {
            Some(part) => Username::from_str(part)?,
            None => return Err(()),
        };
        let password = match parts.next() {
            Some(part) => part.to_owned(),
            None => return Err(()),
        };
        Ok(Auth { username, password })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_correctly() {
        assert_eq!(
            Auth::parse("Basic aW50ZXJsZWRnZXI6cnVzdA==").unwrap(),
            Auth::new("interledger", "rust").unwrap()
        );

        assert_eq!(
            Auth::parse("Bearer interledger%3Arust").unwrap(),
            Auth::new("interledger", "rust").unwrap()
        );

        assert_eq!(
            Auth::parse("Bearer interledger:rust").unwrap(),
            Auth::new("interledger", "rust").unwrap()
        );

        assert!(Auth::parse("SomethingElse asdf").is_err());
        assert!(Auth::parse("Basic asdf").is_err());
        assert!(Auth::parse("Bearer asdf").is_err());

        assert_eq!(
            Auth::new("interledger", "rust").unwrap().to_bearer(),
            "Bearer interledger:rust"
        );
    }

}