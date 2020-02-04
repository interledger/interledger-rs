//! Benchmark packet serialization and deserialization.

use bytes::BytesMut;
use chrono::{DateTime, Utc};
use criterion::{criterion_group, criterion_main, Criterion};
use once_cell::sync::Lazy;
use std::convert::TryFrom;

use ilp::Address;
use ilp::{ErrorCode, Fulfill, Prepare, Reject};
use ilp::{FulfillBuilder, PrepareBuilder, RejectBuilder};
use interledger_packet as ilp;
use std::str::FromStr;

static PREPARE: Lazy<PrepareBuilder<'static>> = Lazy::new(|| PrepareBuilder {
    amount: 107,
    expires_at: DateTime::parse_from_rfc3339("2017-12-23T01:21:40.549Z")
        .unwrap()
        .with_timezone(&Utc)
        .into(),
    execution_condition: b"\
        \x74\xe1\x13\x6d\xc7\x1c\x9e\x5f\x28\x3b\xec\x83\x46\x1c\xbf\x12\
        \x61\xc4\x01\x4f\x72\xd4\x8f\x8d\xd6\x54\x53\xa0\xb8\x4e\x7d\xe1\
    ",
    destination: Address::from_str("example.alice").unwrap(),
    data: b"\
        \x5d\xb3\x43\xfd\xc4\x18\x98\xf6\xdf\x42\x02\x32\x91\x39\xdc\x24\
        \x2d\xd0\xf5\x58\xa8\x11\xb4\x6b\x28\x91\x8f\xda\xb3\x7c\x6c\xb0\
    ",
});
static FULFILL: Lazy<FulfillBuilder<'static>> = Lazy::new(|| FulfillBuilder {
    fulfillment: b"\
        \x11\x7b\x43\x4f\x1a\x54\xe9\x04\x4f\x4f\x54\x92\x3b\x2c\xff\x9e\
        \x4a\x6d\x42\x0a\xe2\x81\xd5\x02\x5d\x7b\xb0\x40\xc4\xb4\xc0\x4a\
    ",
    data: b"\
        \x5d\xb3\x43\xfd\xc4\x18\x98\xf6\xdf\x42\x02\x32\x91\x39\xdc\x24\
        \x2d\xd0\xf5\x58\xa8\x11\xb4\x6b\x28\x91\x8f\xda\xb3\x7c\x6c\xb0\
    ",
});
static EXAMPLE_CONNECTOR: Lazy<Address> =
    Lazy::new(|| Address::from_str("example.connector").unwrap());
static REJECT: Lazy<RejectBuilder<'static>> = Lazy::new(|| RejectBuilder {
    code: ErrorCode::F99_APPLICATION_ERROR,
    message: b"Some error",
    triggered_by: Some(&*EXAMPLE_CONNECTOR),
    data: b"\
        \x5d\xb3\x43\xfd\xc4\x18\x98\xf6\xdf\x42\x02\x32\x91\x39\xdc\x24\
        \x2d\xd0\xf5\x58\xa8\x11\xb4\x6b\x28\x91\x8f\xda\xb3\x7c\x6c\xb0\
    ",
});

fn benchmark_serialize(c: &mut Criterion) {
    let prepare_bytes = BytesMut::from(PREPARE.build());
    c.bench_function("Prepare (serialize)", move |b| {
        b.iter(|| {
            assert_eq!(BytesMut::from(PREPARE.build()), prepare_bytes);
        });
    });

    let fulfill_bytes = BytesMut::from(FULFILL.build());
    c.bench_function("Fulfill (serialize)", move |b| {
        b.iter(|| {
            assert_eq!(BytesMut::from(FULFILL.build()), fulfill_bytes);
        });
    });

    let reject_bytes = BytesMut::from(REJECT.build());
    c.bench_function("Reject (serialize)", move |b| {
        b.iter(|| {
            assert_eq!(BytesMut::from(REJECT.build()), reject_bytes);
        });
    });
}

fn benchmark_deserialize(c: &mut Criterion) {
    let prepare_bytes = BytesMut::from(PREPARE.build());
    c.bench_function("Prepare (deserialize)", move |b| {
        b.iter(|| {
            let parsed = Prepare::try_from(prepare_bytes.clone()).unwrap();
            assert_eq!(parsed.amount(), PREPARE.amount);
            assert_eq!(parsed.destination(), PREPARE.destination);
        });
    });

    let fulfill_bytes = BytesMut::from(FULFILL.build());
    c.bench_function("Fulfill (deserialize)", move |b| {
        b.iter(|| {
            let parsed = Fulfill::try_from(fulfill_bytes.clone()).unwrap();
            assert_eq!(parsed.fulfillment(), FULFILL.fulfillment);
        });
    });

    let reject_bytes = BytesMut::from(REJECT.build());
    c.bench_function("Reject (deserialize)", move |b| {
        b.iter(|| {
            let parsed = Reject::try_from(reject_bytes.clone()).unwrap();
            assert_eq!(parsed.code(), REJECT.code);
        });
    });
}

criterion_group! {
    name = benches;
    config = Criterion::default()
        .sample_size(1000);
    targets =
        benchmark_serialize,
        benchmark_deserialize,
}

criterion_main!(benches);
