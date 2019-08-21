//! # interledger-http
//!
//! Client and server implementations of the [ILP-Over-HTTP](https://github.com/interledger/rfcs/blob/master/0035-ilp-over-http/0035-ilp-over-http.md) bilateral communication protocol.
//! This protocol is intended primarily for server-to-server communication between peers on the Interledger network.

use base64;
use futures::Future;
use interledger_service::Account;
use url::Url;

mod client;
mod server;

/// Originally from [interledger-relay](https://github.com/coilhq/interledger-relay/blob/master/crates/interledger-relay/src/combinators/limit_stream.rs).
mod limit_stream;

pub use self::client::HttpClientService;
pub use self::server::HttpServerService;

pub trait HttpAccount: Account {
    fn get_http_url(&self) -> Option<&Url>;
    fn get_http_auth_token(&self) -> Option<&str>;
}

/// The interface for Stores that can be used with the HttpServerService.
// TODO do we need all of these constraints?
pub trait HttpStore: Clone + Send + Sync + 'static {
    type Account: HttpAccount;

    /// Load account details based on the full HTTP Authorization header
    /// received on the incoming HTTP request.
    fn get_account_from_http_token(
        &self,
        username: &str,
        token: &str,
    ) -> Box<dyn Future<Item = Self::Account, Error = ()> + Send>;
}

// Helpers for parsing authorization methods
pub const BEARER_TOKEN_START: usize = 7;
pub const BASIC_TOKEN_START: usize = 6;

#[derive(Eq, PartialEq, Debug, Clone)]
pub struct Auth {
    username: String,
    password: String,
}

impl Auth {
    pub fn new(username: &str, password: &str) -> Self {
        Auth {
            username: username.to_owned(),
            password: password.to_owned(),
        }
    }

    pub fn username(&self) -> &str {
        &self.username
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
            Some(part) => part.to_owned(),
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
            Auth::parse("Basic ZXZhbjpzY2h3YXJ6").unwrap(),
            Auth::new("evan", "schwarz")
        );

        assert_eq!(
            Auth::parse("Bearer evan%3Aschwarz").unwrap(),
            Auth::new("evan", "schwarz")
        );

        assert_eq!(
            Auth::parse("Bearer evan:schwarz").unwrap(),
            Auth::new("evan", "schwarz")
        );

        assert!(Auth::parse("SomethingElse asdf").is_err());
        assert!(Auth::parse("Basic asdf").is_err());
        assert!(Auth::parse("Bearer asdf").is_err());
    }

}
