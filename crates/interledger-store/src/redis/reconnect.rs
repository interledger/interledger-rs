use futures::{
    future::{err, result, Either},
    Future,
};
use log::{debug, error};
use parking_lot::RwLock;
use redis_crate::{
    aio::{ConnectionLike, SharedConnection},
    Client, ConnectionInfo, RedisError, Value,
};
use std::sync::Arc;

/// Wrapper around a Redis SharedConnection that automatically
/// attempts to reconnect to the DB if the connection is dropped
#[derive(Clone)]
pub struct RedisReconnect {
    pub(crate) redis_info: Arc<ConnectionInfo>,
    pub(crate) conn: Arc<RwLock<SharedConnection>>,
}

impl RedisReconnect {
    pub fn connect(
        redis_info: ConnectionInfo,
    ) -> impl Future<Item = RedisReconnect, Error = RedisError> {
        let redis_info = Arc::new(redis_info);
        get_shared_connection(redis_info.clone()).map(move |conn| RedisReconnect {
            conn: Arc::new(RwLock::new(conn)),
            redis_info,
        })
    }

    pub fn reconnect(self) -> impl Future<Item = Self, Error = RedisError> {
        get_shared_connection(self.redis_info.clone()).and_then(move |shared_connection| {
            (*self.conn.write()) = shared_connection;
            debug!("Reconnected to Redis");
            Ok(self)
        })
    }

    fn get_shared_connection(&self) -> SharedConnection {
        self.conn.read().clone()
    }
}

impl ConnectionLike for RedisReconnect {
    fn get_db(&self) -> i64 {
        self.conn.read().get_db()
    }

    fn req_packed_command(
        self,
        cmd: Vec<u8>,
    ) -> Box<dyn Future<Item = (Self, Value), Error = RedisError> + Send> {
        let clone = self.clone();
        Box::new(
            self.get_shared_connection()
                .req_packed_command(cmd)
                .or_else(move |error| {
                    if error.is_connection_dropped() {
                        debug!("Redis connection was dropped, attempting to reconnect");
                        Either::A(clone.reconnect().then(|_| Err(error)))
                    } else {
                        Either::B(err(error))
                    }
                })
                .map(move |(_conn, res)| (self, res)),
        )
    }

    fn req_packed_commands(
        self,
        cmd: Vec<u8>,
        offset: usize,
        count: usize,
    ) -> Box<dyn Future<Item = (Self, Vec<Value>), Error = RedisError> + Send> {
        let clone = self.clone();
        Box::new(
            self.get_shared_connection()
                .req_packed_commands(cmd, offset, count)
                .or_else(move |error| {
                    if error.is_connection_dropped() {
                        debug!("Redis connection was dropped, attempting to reconnect");
                        Either::A(clone.reconnect().then(|_| Err(error)))
                    } else {
                        Either::B(err(error))
                    }
                })
                .map(|(_conn, res)| (self, res)),
        )
    }
}

fn get_shared_connection(
    redis_info: Arc<ConnectionInfo>,
) -> impl Future<Item = SharedConnection, Error = RedisError> {
    result(Client::open((*redis_info).clone())).and_then(|client| {
        client.get_shared_async_connection().map_err(|e| {
            error!("Error connecting to Redis: {:?}", e);
            e
        })
    })
}
