use crate::stores::{IdempotentEngineData, IdempotentEngineStore};
use bytes::Bytes;
use futures::{future::result, Future};
use http::StatusCode;
use redis::{self, cmd, r#async::SharedConnection, Client, ConnectionInfo, PipelineCommands};
use std::collections::HashMap as SlowHashMap;
use std::str::FromStr;

use log::{debug, error, trace};

pub struct EngineRedisStoreBuilder {
    redis_uri: ConnectionInfo,
}

impl EngineRedisStoreBuilder {
    pub fn new(redis_uri: ConnectionInfo) -> Self {
        EngineRedisStoreBuilder { redis_uri }
    }

    pub fn connect(&self) -> impl Future<Item = EngineRedisStore, Error = ()> {
        result(Client::open(self.redis_uri.clone()))
            .map_err(|err| error!("Error creating Redis client: {:?}", err))
            .and_then(|client| {
                debug!("Connected to redis: {:?}", client);
                client
                    .get_shared_async_connection()
                    .map_err(|err| error!("Error connecting to Redis: {:?}", err))
            })
            .and_then(move |connection| Ok(EngineRedisStore { connection }))
    }
}

/// A Store that uses Redis as its underlying database.
///
/// This store has functionality to handle idempotent data and should be
/// composed in the stores of other Settlement Engines.
#[derive(Clone)]
pub struct EngineRedisStore {
    pub connection: SharedConnection,
}

impl IdempotentEngineStore for EngineRedisStore {
    fn load_idempotent_data(
        &self,
        idempotency_key: String,
    ) -> Box<dyn Future<Item = Option<IdempotentEngineData>, Error = ()> + Send> {
        let idempotency_key_clone = idempotency_key.clone();
        Box::new(
            cmd("HGETALL")
                .arg(idempotency_key.clone())
                .query_async(self.connection.clone())
                .map_err(move |err| {
                    error!(
                        "Error loading idempotency key {}: {:?}",
                        idempotency_key_clone, err
                    )
                })
                .and_then(
                    move |(_connection, ret): (_, SlowHashMap<String, String>)| {
                        if let (Some(status_code), Some(data), Some(input_hash_slice)) = (
                            ret.get("status_code"),
                            ret.get("data"),
                            ret.get("input_hash"),
                        ) {
                            trace!("Loaded idempotency key {:?} - {:?}", idempotency_key, ret);
                            let mut input_hash: [u8; 32] = Default::default();
                            input_hash.copy_from_slice(input_hash_slice.as_ref());
                            Ok(Some((
                                StatusCode::from_str(status_code).unwrap(),
                                Bytes::from(data.clone()),
                                input_hash,
                            )))
                        } else {
                            Ok(None)
                        }
                    },
                ),
        )
    }

    fn save_idempotent_data(
        &self,
        idempotency_key: String,
        input_hash: [u8; 32],
        status_code: StatusCode,
        data: Bytes,
    ) -> Box<dyn Future<Item = (), Error = ()> + Send> {
        let mut pipe = redis::pipe();
        pipe.atomic()
            .cmd("HMSET") // cannot use hset_multiple since data and status_code have different types
            .arg(&idempotency_key)
            .arg("status_code")
            .arg(status_code.as_u16())
            .arg("data")
            .arg(data.as_ref())
            .arg("input_hash")
            .arg(&input_hash)
            .ignore()
            .expire(&idempotency_key, 86400)
            .ignore();
        Box::new(
            pipe.query_async(self.connection.clone())
                .map_err(|err| error!("Error caching: {:?}", err))
                .and_then(move |(_connection, _): (_, Vec<String>)| {
                    trace!(
                        "Cached {:?}: {:?}, {:?}",
                        idempotency_key,
                        status_code,
                        data,
                    );
                    Ok(())
                }),
        )
    }
}

// add tests for idempotency from other store
#[cfg(test)]
mod tests {
    use super::super::test_helpers::store_helpers::{block_on, test_store, IDEMPOTENCY_KEY};
    use super::*;

    #[test]
    fn saves_and_loads_idempotency_key_data_properly() {
        block_on(test_store().and_then(|(store, context)| {
            let input_hash: [u8; 32] = Default::default();
            store
                .save_idempotent_data(
                    IDEMPOTENCY_KEY.clone(),
                    input_hash,
                    StatusCode::OK,
                    Bytes::from("TEST"),
                )
                .map_err(|err| eprintln!("Redis error: {:?}", err))
                .and_then(move |_| {
                    store
                        .load_idempotent_data(IDEMPOTENCY_KEY.clone())
                        .map_err(|err| eprintln!("Redis error: {:?}", err))
                        .and_then(move |data1| {
                            assert_eq!(
                                data1.unwrap(),
                                (StatusCode::OK, Bytes::from("TEST"), input_hash)
                            );
                            let _ = context;

                            store
                                .load_idempotent_data("asdf".to_string())
                                .map_err(|err| eprintln!("Redis error: {:?}", err))
                                .and_then(move |data2| {
                                    assert!(data2.is_none());
                                    let _ = context;
                                    Ok(())
                                })
                        })
                })
        }))
        .unwrap()
    }
}
