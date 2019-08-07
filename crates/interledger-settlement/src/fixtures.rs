use crate::Quantity;
use interledger_packet::Address;
#[cfg(test)]
use lazy_static::lazy_static;
use mockito::Matcher;
use std::str::FromStr;

use crate::test_helpers::TestAccount;

pub static DATA: &str = "DATA_FOR_SETTLEMENT_ENGINE";
pub static BODY: &str = "hi";

lazy_static! {
    pub static ref SETTLEMENT_DATA: Quantity = Quantity::new("100", 18);
    pub static ref TEST_ACCOUNT_0: TestAccount =
        TestAccount::new(0, "http://localhost:1234", "example.account");
    pub static ref SERVICE_ADDRESS: Address = Address::from_str("example.connector").unwrap();
    pub static ref MESSAGES_API: Matcher = Matcher::Regex(r"^/accounts/\d*/messages$".to_string());
    pub static ref SETTLEMENT_API: Matcher =
        Matcher::Regex(r"^/accounts/\d*/settlements$".to_string());
    pub static ref IDEMPOTENCY: Option<String> = Some("AJKJNUjM0oyiAN46".to_string());
}
