use super::*;
use crate::IldcpAccount;
use futures::future::err;
use interledger_service::{incoming_service_fn, Account, IncomingService};

use interledger_packet::{Address, ErrorCode, RejectBuilder};

use lazy_static::lazy_static;
use std::str::FromStr;

lazy_static! {
    pub static ref USERNAME_ACC: TestAccount = TestAccount::new(0, "ausername", None);
    pub static ref ILPADDR_ACC: TestAccount =
        TestAccount::new(0, "anotherusername", Some("example.account"));
    pub static ref SERVICE_ADDRESS: Address = Address::from_str("example.connector").unwrap();
}

#[derive(Debug, Clone)]
pub struct TestAccount {
    pub id: u64,
    pub username: String,
    pub ilp_address: Option<Address>,
    pub no_address: bool,
}

impl Account for TestAccount {
    type AccountId = u64;

    fn id(&self) -> u64 {
        self.id
    }
}

impl IldcpAccount for TestAccount {
    fn asset_code(&self) -> &str {
        "XYZ"
    }

    fn asset_scale(&self) -> u8 {
        9
    }

    fn ilp_address(&self) -> Option<&Address> {
        self.ilp_address.as_ref()
    }

    fn username(&self) -> &str {
        &self.username
    }
}

// Test Service

impl TestAccount {
    pub fn new(id: u64, username: &str, ilp_address: Option<&str>) -> Self {
        let ilp_address = if let Some(ilp_address) = ilp_address {
            Some(Address::from_str(ilp_address).unwrap())
        } else {
            None
        };
        Self {
            id,
            username: username.to_string(),
            ilp_address,
            no_address: false,
        }
    }
}

pub fn test_service() -> IldcpService<impl IncomingService<TestAccount> + Clone, TestAccount> {
    IldcpService::new(
        SERVICE_ADDRESS.clone(),
        incoming_service_fn(|_request| {
            Box::new(err(RejectBuilder {
                code: ErrorCode::F02_UNREACHABLE,
                message: b"No other incoming handler!",
                data: &[],
                triggered_by: Some(&SERVICE_ADDRESS),
            }
            .build()))
        }),
    )
}
