use futures::{future::result, Future};
use redis::{self, r#async::SharedConnection, Client, ConnectionInfo};

use log::{debug, error};

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
