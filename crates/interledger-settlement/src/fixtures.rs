use interledger_packet::Address;
use mockito::Matcher;
use std::str::FromStr;

use crate::test_helpers::TestAccount;

pub static DATA: &str = "DATA_FOR_SETTLEMENT_ENGINE";
pub static BODY: &str = "hi";
pub static SETTLEMENT_BODY: u64 = 100;

lazy_static! {
    pub static ref TEST_ACCOUNT_0: TestAccount =
        TestAccount::new(0, "http://localhost:1234", "peer.settle.xrp-ledger");
    pub static ref SERVICE_ADDRESS: Address = Address::from_str("example.connector").unwrap();
    pub static ref MESSAGES_API: Matcher = Matcher::Regex(r"^/accounts/\d*/messages$".to_string());
    pub static ref SETTLEMENT_API: Matcher =
        Matcher::Regex(r"^/accounts/\d*/settlement$".to_string());
    pub static ref IDEMPOTENCY: Option<String> = Some("AJKJNUjM0oyiAN46".to_string());
}
