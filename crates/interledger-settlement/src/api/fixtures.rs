use crate::core::types::Quantity;
use interledger_packet::Address;
#[cfg(test)]
use lazy_static::lazy_static;
use mockito::Matcher;
use std::str::FromStr;
use uuid::Uuid;

use super::test_helpers::TestAccount;

pub static DATA: &str = "DATA_FOR_SETTLEMENT_ENGINE";
pub static BODY: &str = "hi";
pub static IDEMPOTENCY: &str = "AJKJNUjM0oyiAN46";

lazy_static! {
    pub static ref SETTLEMENT_DATA: Quantity = Quantity::new("100", 18);
    pub static ref TEST_ACCOUNT_0: TestAccount = TestAccount::new(
        Uuid::from_slice(&[0; 16]).unwrap(),
        "http://localhost:1234",
        "example.account"
    );
    pub static ref SERVICE_ADDRESS: Address = Address::from_str("example.connector").unwrap();
    pub static ref MESSAGES_API: Matcher =
        Matcher::Regex(r"^/accounts/[0-9A-Fa-f]{8}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{12}/messages$".to_string());
    pub static ref SETTLEMENT_API: Matcher =
        Matcher::Regex(r"^/accounts/[0-9A-Fa-f]{8}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{12}/settlements$".to_string());
}
