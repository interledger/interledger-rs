use interledger_packet::Address;
use mockito::Matcher;
#[cfg(test)]
use once_cell::sync::Lazy;
use std::str::FromStr;
use uuid::Uuid;

use super::test_helpers::TestAccount;

pub static DATA: &str = "DATA_FOR_SETTLEMENT_ENGINE";
pub static BODY: &str = "hi";
pub static IDEMPOTENCY: &str = "AJKJNUjM0oyiAN46";

pub static TEST_ACCOUNT_0: Lazy<TestAccount> = Lazy::new(|| {
    TestAccount::new(
        Uuid::from_slice(&[0; 16]).unwrap(),
        "http://localhost:1234",
        "example.account",
    )
});
pub static SERVICE_ADDRESS: Lazy<Address> =
    Lazy::new(|| Address::from_str("example.connector").unwrap());
pub static MESSAGES_API: Lazy<Matcher> = Lazy::new(|| {
    Matcher::Regex(r"^/accounts/[0-9A-Fa-f]{8}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{12}/messages$".to_string())
});
