use crate::core::{
    idempotency::{IdempotentData, IdempotentStore},
    scale_with_precision_loss,
    types::{Convert, ConvertDetails, LeftoversStore},
};
use bytes::Bytes;
use futures::TryFutureExt;
use http::StatusCode;
use interledger_errors::{IdempotentStoreError, LeftoversStoreError};
use num_bigint::BigUint;
use redis_crate::{
    self, aio::MultiplexedConnection, AsyncCommands, Client, ConnectionInfo, ErrorKind,
    FromRedisValue, RedisError, RedisWrite, ToRedisArgs, Value,
};
use std::collections::HashMap;
use std::str::FromStr;

use tracing::{debug, error, trace};

use async_trait::async_trait;

#[cfg(test)]
mod test_helpers;

/// Domain separator for leftover amounts
static UNCREDITED_AMOUNT_KEY: &str = "uncredited_engine_settlement_amount";

/// Helper function to get a redis key
fn uncredited_amount_key(account_id: &str) -> String {
    format!("{}:{}", UNCREDITED_AMOUNT_KEY, account_id)
}

/// Builder object to create a Redis connection for the engine
pub struct EngineRedisStoreBuilder {
    redis_url: ConnectionInfo,
}

impl EngineRedisStoreBuilder {
    /// Simple constructor
    pub fn new(redis_url: ConnectionInfo) -> Self {
        EngineRedisStoreBuilder { redis_url }
    }

    /// Connects to the provided redis_url and returns a Redis connection for the Settlement Engine
    pub async fn connect(&self) -> Result<EngineRedisStore, ()> {
        let client = match Client::open(self.redis_url.clone()) {
            Ok(c) => c,
            Err(err) => {
                error!("Error creating Redis client: {:?}", err);
                return Err(());
            }
        };

        let connection = client
            .get_multiplexed_tokio_connection()
            .map_err(|err| error!("Error connecting to Redis: {:?}", err))
            .await?;
        debug!("Connected to redis: {:?}", client);

        Ok(EngineRedisStore { connection })
    }
}

/// A Store that uses Redis as its underlying database.
///
/// This store has functionality to handle idempotent data and should be
/// composed in the stores of other Settlement Engines.
#[derive(Clone)]
pub struct EngineRedisStore {
    pub connection: MultiplexedConnection,
}

#[async_trait]
impl IdempotentStore for EngineRedisStore {
    async fn load_idempotent_data(
        &self,
        idempotency_key: String,
    ) -> Result<Option<IdempotentData>, IdempotentStoreError> {
        let mut connection = self.connection.clone();
        let ret: HashMap<String, String> = connection.hgetall(idempotency_key.clone()).await?;

        if let (Some(status_code), Some(data), Some(input_hash_slice)) = (
            ret.get("status_code"),
            ret.get("data"),
            ret.get("input_hash"),
        ) {
            trace!("Loaded idempotency key {:?} - {:?}", idempotency_key, ret);
            let mut input_hash: [u8; 32] = Default::default();
            input_hash.copy_from_slice(input_hash_slice.as_ref());
            Ok(Some(IdempotentData::new(
                StatusCode::from_str(status_code).unwrap(),
                Bytes::from(data.clone()),
                input_hash,
            )))
        } else {
            Ok(None)
        }
    }

    async fn save_idempotent_data(
        &self,
        idempotency_key: String,
        input_hash: [u8; 32],
        status_code: StatusCode,
        data: Bytes,
    ) -> Result<(), IdempotentStoreError> {
        let mut pipe = redis_crate::pipe();
        let mut connection = self.connection.clone();
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
        pipe.query_async(&mut connection).await?;
        trace!(
            "Cached {:?}: {:?}, {:?}",
            idempotency_key,
            status_code,
            data,
        );
        Ok(())
    }
}

/// Helper datatype for storing and loading quantities of a number with different scales
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
    /// Iterates over all values because in this case it's making
    /// an lrange call. This returns all the tuple elements in 1 array, and
    /// it cannot differentiate between 1 AmountWithScale value or multiple
    /// ones. This looks like a limitation of redis.rs
    fn parse_multi_values(items: &[Value]) -> Option<Self> {
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

#[async_trait]
impl LeftoversStore for EngineRedisStore {
    type AccountId = String;
    type AssetType = BigUint;

    async fn get_uncredited_settlement_amount(
        &self,
        account_id: Self::AccountId,
    ) -> Result<(Self::AssetType, u8), LeftoversStoreError> {
        let mut connection = self.connection.clone();
        let amount: AmountWithScale = connection
            .lrange(uncredited_amount_key(&account_id), 0, -1)
            .await?;
        Ok((amount.num, amount.scale))
    }

    async fn save_uncredited_settlement_amount(
        &self,
        account_id: Self::AccountId,
        uncredited_settlement_amount: (Self::AssetType, u8),
    ) -> Result<(), LeftoversStoreError> {
        trace!(
            "Saving uncredited_settlement_amount {:?} {:?}",
            account_id,
            uncredited_settlement_amount
        );
        // We store these amounts as lists of strings
        // because we cannot do BigNumber arithmetic in the store
        // When loading the amounts, we convert them to the appropriate data
        // type and sum them up.
        let mut connection = self.connection.clone();
        connection
            .rpush(
                uncredited_amount_key(&account_id),
                AmountWithScale {
                    num: uncredited_settlement_amount.0,
                    scale: uncredited_settlement_amount.1,
                },
            )
            .await?;

        Ok(())
    }

    async fn load_uncredited_settlement_amount(
        &self,
        account_id: Self::AccountId,
        local_scale: u8,
    ) -> Result<Self::AssetType, LeftoversStoreError> {
        let connection = self.connection.clone();
        trace!("Loading uncredited_settlement_amount {:?}", account_id);
        let amount = self
            .get_uncredited_settlement_amount(account_id.clone())
            .await?;
        // scale the amount from the max scale to the local scale, and then
        // save any potential leftovers to the store
        let (scaled_amount, precision_loss) =
            scale_with_precision_loss(amount.0, local_scale, amount.1);

        let mut pipe = redis_crate::pipe();
        pipe.atomic();
        pipe.del(uncredited_amount_key(&account_id)).ignore();
        pipe.rpush(
            uncredited_amount_key(&account_id),
            AmountWithScale {
                num: precision_loss,
                scale: std::cmp::max(local_scale, amount.1),
            },
        )
        .ignore();

        pipe.query_async(&mut connection.clone()).await?;

        Ok(scaled_amount)
    }

    async fn clear_uncredited_settlement_amount(
        &self,
        account_id: Self::AccountId,
    ) -> Result<(), LeftoversStoreError> {
        trace!("Clearing uncredited_settlement_amount {:?}", account_id,);
        let mut connection = self.connection.clone();
        connection.del(uncredited_amount_key(&account_id)).await?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_helpers::{test_store, IDEMPOTENCY_KEY};

    mod idempotency {
        use super::*;

        #[tokio::test]
        async fn saves_and_loads_idempotency_key_data_properly() {
            // The context must be loaded into scope
            let (store, _context) = test_store().await.unwrap();
            let input_hash: [u8; 32] = Default::default();
            store
                .save_idempotent_data(
                    IDEMPOTENCY_KEY.clone(),
                    input_hash,
                    StatusCode::OK,
                    Bytes::from("TEST"),
                )
                .await
                .unwrap();

            let data1 = store
                .load_idempotent_data(IDEMPOTENCY_KEY.clone())
                .await
                .unwrap();
            assert_eq!(
                data1.unwrap(),
                IdempotentData::new(StatusCode::OK, Bytes::from("TEST"), input_hash)
            );

            let data2 = store
                .load_idempotent_data("asdf".to_string())
                .await
                .unwrap();
            assert!(data2.is_none());
        }
    }
}
