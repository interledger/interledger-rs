use mockito;
use serde_json::json;
use std::collections::HashMap;
use std::str::FromStr;
use std::time::Duration;
use web3::contract::{Contract, Options};
use web3::{
    api::Web3,
    futures::future::Future,
    transports::Http,
    types::{H256, U256},
};

use super::utils::*;
use interledger_settlement::Quantity;
use std::iter::FromIterator;

use interledger_settlement_engines::{
    engines::ethereum_ledger::{EthereumAddresses as Addresses, EthereumStore},
    SettlementEngine,
};

use lazy_static::lazy_static;

lazy_static! {
    pub static ref ALICE_PK: String =
        String::from("380eb0f3d505f087e438eca80bc4df9a7faa24f868e69fc0440261a0fc0567dc");
    pub static ref BOB_PK: String =
        String::from("cc96601bc52293b53c4736a12af9130abf347669b3813f9ec4cafdf6991b087e");
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
}

#[test]
fn test_send_erc20() {
    let ganache_port = 8546;
    let mut ganache_pid = start_ganache(ganache_port);
    let _ = env_logger::try_init();
    let alice = ALICE.clone();
    let bob = BOB.clone();
    let (eloop, transport) = Http::new(&format!("http://localhost:{}", ganache_port)).unwrap();
    eloop.into_remote();
    let web3 = Web3::new(transport);
    // deploy erc20 contract
    let erc20_bytecode = include_str!("./fixtures/erc20.code");
    let contract = Contract::deploy(web3.eth(), include_bytes!("./fixtures/erc20_abi.json"))
        .unwrap()
        .confirmations(0)
        .options(Options::with(|opt| {
            opt.gas_price = Some(5.into());
            opt.gas = Some(2_000_000.into());
        }))
        .execute(
            erc20_bytecode,
            U256::from_dec_str("1000000000000000000000").unwrap(),
            alice.address,
        )
        .expect("Correct parameters are passed to the constructor.")
        .wait()
        .unwrap();

    let token_address = contract.address();

    let alice_store = test_store(ALICE.clone(), false, false, true);
    alice_store
        .save_account_addresses(HashMap::from_iter(vec![(
            "0".to_string(),
            Addresses {
                own_address: bob.address,
                token_address: Some(token_address),
            },
        )]))
        .wait()
        .unwrap();

    let bob_store = test_store(bob.clone(), false, false, true);
    bob_store
        .save_account_addresses(HashMap::from_iter(vec![(
            "42".to_string(),
            Addresses {
                own_address: alice.address,
                token_address: Some(token_address),
            },
        )]))
        .wait()
        .unwrap();

    let bob_mock = mockito::mock("POST", "/accounts/42/settlements")
        .match_body(mockito::Matcher::JsonString(
            json!(Quantity::new(100_000_000_000u64, 18)).to_string(),
        ))
        .with_status(200)
        .with_body(json!(Quantity::new(100, 9)).to_string())
        .create();

    let bob_connector_url = mockito::server_url();
    let _bob_engine = test_engine(
        bob_store.clone(),
        BOB_PK.clone(),
        0,
        &bob_connector_url,
        8546,
        Some(token_address),
        true,
    );

    let alice_engine = test_engine(
        alice_store.clone(),
        ALICE_PK.clone(),
        0,
        "http://127.0.0.1:9999",
        8546,
        Some(token_address),
        false, // alice sends the transaction to bob (set it up so that she doesn't listen for inc txs)
    );

    // 100 Gwei
    let ret = block_on(alice_engine.send_money(bob.id.to_string(), Quantity::new(100, 9))).unwrap();
    assert_eq!(ret.0.as_u16(), 200);
    assert_eq!(ret.1, "OK");

    // wait a few seconds so that the receiver's engine that does the polling
    std::thread::sleep(Duration::from_millis(2000));

    // did token balances update correctly?
    let token_balance = |address| {
        let balance: U256 = contract
            .query("balanceOf", address, None, Options::default(), None)
            .wait()
            .unwrap();
        balance
    };
    let alice_balance = token_balance(alice.address);
    let bob_balance = token_balance(bob.address);
    assert_eq!(
        alice_balance,
        U256::from_dec_str("999999999900000000000").unwrap()
    );
    assert_eq!(bob_balance, U256::from_dec_str("100000000000").unwrap()); // 100 + 9 0's for the Gwei conversion

    ganache_pid.kill().unwrap(); // kill ganache since it's no longer needed
    bob_mock.assert();
}

#[test]
fn test_send_eth() {
    let _ = env_logger::try_init();
    let alice = ALICE.clone();
    let bob = BOB.clone();

    let alice_store = test_store(ALICE.clone(), false, false, true);
    alice_store
        .save_account_addresses(HashMap::from_iter(vec![(
            "0".to_string(),
            Addresses {
                own_address: bob.address,
                token_address: None,
            },
        )]))
        .wait()
        .unwrap();

    let bob_store = test_store(bob.clone(), false, false, true);
    bob_store
        .save_account_addresses(HashMap::from_iter(vec![(
            "42".to_string(),
            Addresses {
                own_address: alice.address,
                token_address: None,
            },
        )]))
        .wait()
        .unwrap();

    let mut ganache_pid = start_ganache(8545);

    let bob_mock = mockito::mock("POST", "/accounts/42/settlements")
        .match_body(mockito::Matcher::JsonString(
            json!(Quantity::new(100_000_000_000u64, 18)).to_string(),
        ))
        .with_status(200)
        .with_body(json!(Quantity::new(100, 9)).to_string())
        .create();

    let bob_connector_url = mockito::server_url();
    let _bob_engine = test_engine(
        bob_store.clone(),
        BOB_PK.clone(),
        0,
        &bob_connector_url,
        8545,
        None,
        true,
    );

    let alice_engine = test_engine(
        alice_store.clone(),
        ALICE_PK.clone(),
        0,
        "http://127.0.0.1:9999",
        8545,
        None,
        false, // alice sends the transaction to bob (set it up so that she doesn't listen for inc txs)
    );

    let ret = block_on(alice_engine.send_money(bob.id.to_string(), Quantity::new(100, 9))).unwrap();
    assert_eq!(ret.0.as_u16(), 200);
    assert_eq!(ret.1, "OK");

    std::thread::sleep(Duration::from_millis(2000)); // wait a few seconds so that the receiver's engine that does the polling

    let (eloop, transport) = Http::new("http://localhost:8545").unwrap();
    eloop.into_remote();
    let web3 = Web3::new(transport);
    let alice_balance = web3.eth().balance(alice.address, None).wait().unwrap();
    let bob_balance = web3.eth().balance(bob.address, None).wait().unwrap();
    let expected_alice = U256::from_dec_str("99999579900000000000").unwrap(); // 99ether - 21k gas - 100 gwei
    let expected_bob = U256::from_dec_str("100000000100000000000").unwrap(); // 100 ether + 100 gwei
    assert_eq!(alice_balance, expected_alice);
    assert_eq!(bob_balance, expected_bob);

    ganache_pid.kill().unwrap(); // kill ganache since it's no longer needed
    bob_mock.assert();
}

#[test]
fn saves_leftovers() {
    // dummy tx_hashes to avoid making idempotent calls
    let tx_hash1 =
        H256::from_str("5ad3b56557dab5994c264ca17e2e08816341be2e6649ee6b2b1141006bfd347e").unwrap();
    let tx_hash2 =
        H256::from_str("5ad3b56557dab5994c264ca17e2e08816341be2e6649ee6b2b1141006bfd3472").unwrap();
    let tx_hash3 =
        H256::from_str("5ad3b56557dab5994c264ca17e2e08816341be2e6649ee6b2b1141006bfd3471").unwrap();

    let bob = BOB.clone();
    let alice = ALICE.clone();
    let bob_store = test_store(bob.clone(), false, false, true);
    bob_store
        .save_account_addresses(HashMap::from_iter(vec![(
            "42".to_string(),
            Addresses {
                own_address: alice.address,
                token_address: None,
            },
        )]))
        .wait()
        .unwrap();

    let mut ganache_pid = start_ganache(8547);

    // helper for customizing connector return values and making sure the
    // engine makes POSTs with the correct arguments
    let connector_mock = |received, ret| {
        // the connector will return any amount it receives with 2 less 0s
        let received = json!(Quantity::new(received, 18)).to_string();
        let ret = json!(Quantity::new(ret, 16)).to_string();
        mockito::mock("POST", "/accounts/42/settlements")
            .match_body(mockito::Matcher::JsonString(received))
            .with_status(200)
            .with_body(ret)
    };

    let full_amount = "100000000000";
    let full_return = "1000000000";
    let amount_with_leftovers = "110000000000";
    let full_return_with_leftovers = "1100000000";
    let partial_return = "900000000";

    // Bob's connector is initially set up to not return the full amount
    let bob_connector = connector_mock(full_amount, partial_return).create();
    // initialize the engine
    let bob_connector_url = mockito::server_url();
    let bob_engine = test_engine(
        bob_store.clone(),
        BOB_PK.clone(),
        0,
        &bob_connector_url,
        8547,
        None,
        true,
    );

    // the call the engine makes when it picks up an on-chain event
    let ping_connector = |idempotency| {
        block_on(bob_engine.notify_connector(
            "42".to_string(),
            full_amount.to_string(),
            idempotency,
        ))
        .unwrap()
    };

    ping_connector(tx_hash1);
    bob_connector.assert();

    // for whatever reason, the connector now will return the full amount
    // (but our engine must also send the uncredited amount from the
    // previous call)
    let bob_connector = connector_mock(amount_with_leftovers, full_return_with_leftovers).create();
    ping_connector(tx_hash2);
    bob_connector.assert();

    // after the leftovers have been cleared, our engine continues normally
    let bob_connector = connector_mock(full_amount, full_return).create();
    ping_connector(tx_hash3);
    bob_connector.assert();

    ganache_pid.kill().unwrap(); // kill ganache since it's no longer needed
}
