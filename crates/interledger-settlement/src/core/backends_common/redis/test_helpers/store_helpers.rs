use super::redis_helpers::TestContext;
use crate::core::backends_common::redis::{EngineRedisStore, EngineRedisStoreBuilder};

use lazy_static::lazy_static;

lazy_static! {
    pub static ref IDEMPOTENCY_KEY: String = String::from("abcd");
}

pub async fn test_store() -> Result<(EngineRedisStore, TestContext), ()> {
    let context = TestContext::new();
    let store = EngineRedisStoreBuilder::new(context.get_client_connection_info())
        .connect()
        .await?;
    Ok((store, context))
}
