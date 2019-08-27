use super::utils::TestAccount;
use lazy_static::lazy_static;
use mockito::Matcher;

lazy_static! {
    pub static ref ALICE: TestAccount = TestAccount::new(
        "1".to_string(),
        "3cdb3d9e1b74692bb1e3bb5fc81938151ca64b02",
        "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee"
    );
    pub static ref BOB: TestAccount = TestAccount::new(
        "0".to_string(),
        "9b925641c5ef3fd86f63bff2da55a0deeafd1263",
        "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee"
    );
    pub static ref SETTLEMENT_API: Matcher =
        Matcher::Regex(r"^/accounts/\d*/settlements$".to_string());
    pub static ref MESSAGES_API: Matcher = Matcher::Regex(r"^/accounts/\d*/messages$".to_string());
}
