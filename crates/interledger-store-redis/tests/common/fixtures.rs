use interledger_api::AccountDetails;
use interledger_packet::Address;
use interledger_service::Username;
use lazy_static::lazy_static;
use std::str::FromStr;

lazy_static! {
    // We are dylan starting a connection with all these accounts
    pub static ref ACCOUNT_DETAILS_0: AccountDetails = AccountDetails {
        ilp_address: Some(Address::from_str("example.alice").unwrap()),
        username: Username::from_str("alice").unwrap(),
        asset_scale: 6,
        asset_code: "XYZ".to_string(),
        max_packet_amount: 1000,
        min_balance: Some(-1000),
        http_endpoint: Some("http://example.com/ilp".to_string()),
        http_incoming_token: Some("incoming_auth_token".to_string()),
        http_outgoing_token: Some("dylan:outgoing_auth_token".to_string()),
        btp_uri: Some("btp+ws://dylan:btp_token@example.com/btp".to_string()),
        btp_incoming_token: Some("btp_token".to_string()),
        settle_threshold: Some(0),
        settle_to: Some(-1000),
        routing_relation: Some("Parent".to_owned()),
        round_trip_time: None,
        amount_per_minute_limit: Some(1000),
        packets_per_minute_limit: Some(2),
        settlement_engine_url: None,
    };
    pub static ref ACCOUNT_DETAILS_1: AccountDetails = AccountDetails {
        ilp_address: None,
        username: Username::from_str("bob").unwrap(),
        asset_scale: 9,
        asset_code: "ABC".to_string(),
        max_packet_amount: 1_000_000,
        min_balance: Some(0),
        http_endpoint: Some("http://example.com/ilp".to_string()),
        // incoming token has is the account's username concatenated wiht the password
        http_incoming_token: Some("incoming_auth_token".to_string()),
        http_outgoing_token: Some("dylan:outgoing_auth_token".to_string()),
        btp_uri: Some("btp+ws://dylan:btp_token@example.com/btp".to_string()),
        btp_incoming_token: Some("other_btp_token".to_string()),
        settle_threshold: Some(0),
        settle_to: Some(-1000),
        routing_relation: Some("Child".to_owned()),
        round_trip_time: None,
        amount_per_minute_limit: Some(1000),
        packets_per_minute_limit: Some(20),
        settlement_engine_url: None,
    };
    pub static ref ACCOUNT_DETAILS_2: AccountDetails = AccountDetails {
        ilp_address: None,
        username: Username::from_str("charlie").unwrap(),
        asset_scale: 9,
        asset_code: "XRP".to_string(),
        max_packet_amount: 1000,
        min_balance: Some(0),
        http_endpoint: None,
        http_incoming_token: None,
        http_outgoing_token: None,
        btp_uri: None,
        btp_incoming_token: None,
        settle_threshold: Some(0),
        settle_to: None,
        routing_relation: None,
        round_trip_time: None,
        amount_per_minute_limit: None,
        packets_per_minute_limit: None,
        settlement_engine_url: None,
    };
}
