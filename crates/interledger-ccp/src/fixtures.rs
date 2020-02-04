/* kcov-ignore-start */
use crate::packet::*;
use bytes::Bytes;
use hex;
use interledger_packet::Address;
#[cfg(test)]
use once_cell::sync::Lazy;
use std::str::FromStr;

pub static CONTROL_REQUEST_SERIALIZED: Lazy<Vec<u8>> = Lazy::new(|| {
    hex::decode("0c6c0000000000000000323031353036313630303031303030303066687aadf862bd776c8fc18b8e9f8e20089714856ee233b3902a591d0d5f292512706565722e726f7574652e636f6e74726f6c1f0170d1a134a0df4f47964f6e19e2ab379000000020010203666f6f03626172").unwrap()
});
pub static CONTROL_REQUEST: Lazy<RouteControlRequest> = Lazy::new(|| RouteControlRequest {
    mode: Mode::Sync,
    last_known_routing_table_id: [
        112, 209, 161, 52, 160, 223, 79, 71, 150, 79, 110, 25, 226, 171, 55, 144,
    ], // "70d1a134-a0df-4f47-964f-6e19e2ab3790"
    last_known_epoch: 32,
    features: vec!["foo".to_string(), "bar".to_string()],
});

pub static UPDATE_REQUEST_SIMPLE_SERIALIZED: Lazy<Vec<u8>> = Lazy::new(|| {
    hex::decode("0c7e0000000000000000323031353036313630303031303030303066687aadf862bd776c8fc18b8e9f8e20089714856ee233b3902a591d0d5f292511706565722e726f7574652e7570646174653221e55f8eabcd4e979ab9bf0ff00a224c000000340000003400000034000075300d6578616d706c652e616c69636501000100").unwrap()
});
pub static UPDATE_REQUEST_SIMPLE: Lazy<RouteUpdateRequest> = Lazy::new(|| RouteUpdateRequest {
    routing_table_id: [
        33, 229, 95, 142, 171, 205, 78, 151, 154, 185, 191, 15, 240, 10, 34, 76,
    ], // '21e55f8e-abcd-4e97-9ab9-bf0ff00a224c'
    current_epoch_index: 52,
    from_epoch_index: 52,
    to_epoch_index: 52,
    hold_down_time: 30000,
    speaker: Address::from_str("example.alice").unwrap(),
    new_routes: Vec::new(),
    withdrawn_routes: Vec::new(),
});

pub static UPDATE_REQUEST_COMPLEX_SERIALIZED: Lazy<Vec<u8>> = Lazy::new(|| {
    hex::decode("0c8201520000000000000000323031353036313630303031303030303066687aadf862bd776c8fc18b8e9f8e20089714856ee233b3902a591d0d5f292511706565722e726f7574652e757064617465820104bffbf6ad0ddc4d3ba1e5b4f0537365bd000000340000002e00000032000075300d6578616d706c652e616c69636501020f6578616d706c652e7072656669783101010f6578616d706c652e707265666978317a6c7d85867c46a2fabfad1afa7a4a5e229ce574fcce63f5edeedfc03f8468ea01000f6578616d706c652e707265666978320102126578616d706c652e636f6e6e6563746f72310f6578616d706c652e707265666978322b08e53fbcc17c5f1bd54ae0d9ad7ba39a5f9a7b126ca9b5c0945609a35324cc01025000000b68656c6c6f20776f726c64e0000104a0a0a0a001020f6578616d706c652e707265666978330f6578616d706c652e70726566697834").unwrap()
});
pub static UPDATE_REQUEST_COMPLEX: Lazy<RouteUpdateRequest> = Lazy::new(|| RouteUpdateRequest {
    routing_table_id: [
        191, 251, 246, 173, 13, 220, 77, 59, 161, 229, 180, 240, 83, 115, 101, 189,
    ], // 'bffbf6ad-0ddc-4d3b-a1e5-b4f0537365bd'
    current_epoch_index: 52,
    from_epoch_index: 46,
    to_epoch_index: 50,
    hold_down_time: 30000,
    speaker: Address::from_str("example.alice").unwrap(),
    new_routes: vec![
        Route {
            prefix: "example.prefix1".to_string(),
            path: vec!["example.prefix1".to_string()],
            auth: [
                122, 108, 125, 133, 134, 124, 70, 162, 250, 191, 173, 26, 250, 122, 74, 94, 34,
                156, 229, 116, 252, 206, 99, 245, 237, 238, 223, 192, 63, 132, 104, 234,
            ],
            props: Vec::new(),
        },
        Route {
            prefix: "example.prefix2".to_string(),
            path: vec![
                "example.connector1".to_string(),
                "example.prefix2".to_string(),
            ],
            auth: [
                43, 8, 229, 63, 188, 193, 124, 95, 27, 213, 74, 224, 217, 173, 123, 163, 154, 95,
                154, 123, 18, 108, 169, 181, 192, 148, 86, 9, 163, 83, 36, 204,
            ],
            props: vec![
                RouteProp {
                    is_optional: false,
                    is_transitive: true,
                    is_partial: false,
                    is_utf8: true,
                    id: 0,
                    value: Bytes::from("hello world"),
                },
                RouteProp {
                    is_optional: true,
                    is_transitive: true,
                    is_partial: true,
                    is_utf8: false,
                    id: 1,
                    value: Bytes::from(hex::decode("a0a0a0a0").unwrap()),
                },
            ],
        },
    ],
    withdrawn_routes: vec!["example.prefix3".to_string(), "example.prefix4".to_string()],
});
/* kcov-ignore-end */
