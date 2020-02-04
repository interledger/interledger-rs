use super::redis_helpers::TestContext;
use crate::core::backends_common::redis::{EngineRedisStore, EngineRedisStoreBuilder};

use once_cell::sync::Lazy;

pub static IDEMPOTENCY_KEY: Lazy<String> = Lazy::new(|| String::from("abcd"));

pub async fn test_store() -> Result<(EngineRedisStore, TestContext), ()> {
    let context = TestContext::new();
    let store = EngineRedisStoreBuilder::new(context.get_client_connection_info())
        .connect()
        .await?;
    Ok((store, context))
}
