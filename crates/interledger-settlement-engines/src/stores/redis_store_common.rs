use crate::stores::{IdempotentEngineData, IdempotentEngineStore};
use bytes::Bytes;
use futures::{future::result, Future};
use http::StatusCode;
use interledger_settlement::{scale_with_precision_loss, Convert, ConvertDetails, LeftoversStore};
use num_bigint::BigUint;
use redis::{
    self, aio::SharedConnection, cmd, Client, ConnectionInfo, ErrorKind, FromRedisValue,
    PipelineCommands, RedisError, RedisWrite, ToRedisArgs, Value,
};
use std::collections::HashMap as SlowHashMap;
use std::str::FromStr;

use log::{debug, error, trace};

static UNCREDITED_AMOUNT_KEY: &str = "uncredited_engine_settlement_amount";
fn uncredited_amount_key(account_id: String) -> String {
    format!("{}:{}", UNCREDITED_AMOUNT_KEY, account_id)
}

pub struct EngineRedisStoreBuilder {
    redis_url: ConnectionInfo,
}

impl EngineRedisStoreBuilder {
    pub fn new(redis_url: ConnectionInfo) -> Self {
        EngineRedisStoreBuilder { redis_url }
    }

    pub fn connect(&self) -> impl Future<Item = EngineRedisStore, Error = ()> {
        result(Client::open(self.redis_url.clone()))
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

#[derive(Debug, Clone)]
struct AmountWithScale {
    num: BigUint,
    scale: u8,
}

impl ToRedisArgs for AmountWithScale {
    fn write_redis_args<W>(&self, out: &mut W)
    where
        W: ?Sized + RedisWrite,
    {
        let mut rv = Vec::new();
        self.num.to_string().write_redis_args(&mut rv);
        self.scale.to_string().write_redis_args(&mut rv);
        ToRedisArgs::make_arg_vec(&rv, out);
    }
}

impl AmountWithScale {
    fn parse_multi_values(items: &[Value]) -> Option<Self> {
        // We have to iterate over all values because in this case we're making
        // an lrange call. This returns all the tuple elements in 1 array, and
        // it cannot differentiate between 1 AmountWithScale value or multiple
        // ones. This looks like a limitation of redis.rs
        let len = items.len();
        let mut iter = items.iter();

        let mut max_scale = 0;
        let mut amounts = Vec::new();
        // if redis.rs could parse this properly, we could remove this loop,
        // take 2 elements from the items iterator and return. Then we'd perform
        // the summation and scaling in the consumer of the returned vector.
        for _ in (0..len).step_by(2) {
            let num: String = match iter.next().map(FromRedisValue::from_redis_value) {
                Some(Ok(n)) => n,
                _ => return None,
            };
            let num = match BigUint::from_str(&num) {
                Ok(a) => a,
                Err(_) => return None,
            };

            let scale: u8 = match iter.next().map(FromRedisValue::from_redis_value) {
                Some(Ok(c)) => c,
                _ => return None,
            };

            if scale > max_scale {
                max_scale = scale;
            }
            amounts.push((num, scale));
        }

        // We must scale them to the largest scale, and then add them together
        let mut sum = BigUint::from(0u32);
        for amount in &amounts {
            sum += amount
                .0
                .normalize_scale(ConvertDetails {
                    from: amount.1,
                    to: max_scale,
                })
                .unwrap();
        }

        Some(AmountWithScale {
            num: sum,
            scale: max_scale,
        })
    }
}

impl FromRedisValue for AmountWithScale {
    fn from_redis_value(v: &Value) -> Result<Self, RedisError> {
        if let Value::Bulk(ref items) = *v {
            if let Some(result) = Self::parse_multi_values(items) {
                return Ok(result);
            }
        }
        Err(RedisError::from((
            ErrorKind::TypeError,
            "Cannot parse amount with scale",
        )))
    }
}

impl LeftoversStore for EngineRedisStore {
    type AccountId = String;
    type AssetType = BigUint;

    fn get_uncredited_settlement_amount(
        &self,
        account_id: Self::AccountId,
    ) -> Box<dyn Future<Item = (Self::AssetType, u8), Error = ()> + Send> {
        Box::new(
            cmd("LRANGE")
                .arg(uncredited_amount_key(account_id.to_string()))
                .arg(0)
                .arg(-1)
                .query_async(self.connection.clone())
                .map_err(move |err| error!("Error getting uncredited_settlement_amount {:?}", err))
                .and_then(move |(_, amount): (_, AmountWithScale)| Ok((amount.num, amount.scale))),
        )
    }

    fn save_uncredited_settlement_amount(
        &self,
        account_id: Self::AccountId,
        uncredited_settlement_amount: (Self::AssetType, u8),
    ) -> Box<dyn Future<Item = (), Error = ()> + Send> {
        trace!(
            "Saving uncredited_settlement_amount {:?} {:?}",
            account_id,
            uncredited_settlement_amount
        );
        Box::new(
            // We store these amounts as lists of strings
            // because we cannot do BigNumber arithmetic in the store
            // When loading the amounts, we convert them to the appropriate data
            // type and sum them up.
            cmd("RPUSH")
                .arg(uncredited_amount_key(account_id))
                .arg(AmountWithScale {
                    num: uncredited_settlement_amount.0,
                    scale: uncredited_settlement_amount.1,
                })
                .query_async(self.connection.clone())
                .map_err(move |err| error!("Error saving uncredited_settlement_amount: {:?}", err))
                .and_then(move |(_conn, _ret): (_, Value)| Ok(())),
        )
    }

    fn load_uncredited_settlement_amount(
        &self,
        account_id: Self::AccountId,
        local_scale: u8,
    ) -> Box<dyn Future<Item = Self::AssetType, Error = ()> + Send> {
        let connection = self.connection.clone();
        trace!("Loading uncredited_settlement_amount {:?}", account_id);
        Box::new(
            self.get_uncredited_settlement_amount(account_id.clone())
                .and_then(move |amount| {
                    // scale the amount from the max scale to the local scale, and then
                    // save any potential leftovers to the store
                    let (scaled_amount, precision_loss) =
                        scale_with_precision_loss(amount.0, local_scale, amount.1);
                    let mut pipe = redis::pipe();
                    pipe.atomic();
                    pipe.del(uncredited_amount_key(account_id.clone())).ignore();
                    pipe.rpush(
                        uncredited_amount_key(account_id),
                        AmountWithScale {
                            num: precision_loss,
                            scale: std::cmp::max(local_scale, amount.1),
                        },
                    )
                    .ignore();

                    pipe.query_async(connection.clone())
                        .map_err(move |err| {
                            error!("Error saving uncredited_settlement_amount: {:?}", err)
                        })
                        .and_then(move |(_conn, _ret): (_, Value)| Ok(scaled_amount))
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
