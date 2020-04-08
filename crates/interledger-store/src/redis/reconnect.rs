use futures::future::{FutureExt, TryFutureExt};
use parking_lot::RwLock;
use redis_crate::{
    aio::{ConnectionLike, MultiplexedConnection},
    Client, Cmd, ConnectionInfo, Pipeline, RedisError, RedisFuture, Value,
};
use std::sync::Arc;
use tracing::{debug, error};

type Result<T> = std::result::Result<T, RedisError>;

/// Wrapper around a Redis MultiplexedConnection that automatically
/// attempts to reconnect to the DB if the connection is dropped
#[derive(Clone)]
pub struct RedisReconnect {
    pub(crate) redis_info: Arc<ConnectionInfo>,
    pub(crate) conn: Arc<RwLock<MultiplexedConnection>>,
}

async fn get_shared_connection(redis_info: Arc<ConnectionInfo>) -> Result<MultiplexedConnection> {
    let client = Client::open((*redis_info).clone())?;
    client
        .get_multiplexed_tokio_connection()
        .map_err(|e| {
            error!("Error connecting to Redis: {:?}", e);
            e
        })
        .await
}

impl RedisReconnect {
    /// Connects to redis with the provided [`ConnectionInfo`](redis_crate::ConnectionInfo)
    pub async fn connect(redis_info: ConnectionInfo) -> Result<RedisReconnect> {
        let redis_info = Arc::new(redis_info);
        let conn = get_shared_connection(redis_info.clone()).await?;
        Ok(RedisReconnect {
            conn: Arc::new(RwLock::new(conn)),
            redis_info,
        })
    }

    /// Reconnects to redis
    pub async fn reconnect(self) -> Result<Self> {
        let shared_connection = get_shared_connection(self.redis_info.clone()).await?;
        (*self.conn.write()) = shared_connection;
        debug!("Reconnected to Redis");
        Ok(self)
    }

    fn get_shared_connection(&self) -> MultiplexedConnection {
        self.conn.read().clone()
    }
}

impl ConnectionLike for RedisReconnect {
    fn get_db(&self) -> i64 {
        self.conn.read().get_db()
    }

    fn req_packed_command<'a>(&'a mut self, cmd: &'a Cmd) -> RedisFuture<'a, Value> {
        // This is how it is implemented in the redis-rs repository
        (async move {
            let mut connection = self.get_shared_connection();
            match connection.req_packed_command(cmd).await {
                Ok(res) => Ok(res),
                Err(error) => {
                    if error.is_connection_dropped() {
                        debug!("Redis connection was dropped, attempting to reconnect");
                        // TODO: Is this correct syntax? Otherwise we get an unused result warning
                        let _ = self.clone().reconnect().await;
                    }
                    Err(error)
                }
            }
        })
        .boxed()
    }

    fn req_packed_commands<'a>(
        &'a mut self,
        cmd: &'a Pipeline,
        offset: usize,
        count: usize,
    ) -> RedisFuture<'a, Vec<Value>> {
        // This is how it is implemented in the redis-rs repository
        (async move {
            let mut connection = self.get_shared_connection();
            match connection.req_packed_commands(cmd, offset, count).await {
                Ok(res) => Ok(res),
                Err(error) => {
                    if error.is_connection_dropped() {
                        debug!("Redis connection was dropped, attempting to reconnect");
                        // TODO: Is this correct syntax? Otherwise we get an unused result warning
                        let _ = self.clone().reconnect().await;
                    }
                    Err(error)
                }
            }
        })
        .boxed()
    }
}
