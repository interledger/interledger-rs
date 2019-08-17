use futures::future::Future;

pub mod redis_ethereum_ledger;
pub mod redis_store_common;

#[cfg(test)]
pub mod test_helpers;

pub trait LeftoversStore {
    type AssetType;

    /// Saves the leftover data
    fn save_uncredited_settlement_amount(
        &self,
        account_id: String,
        uncredited_settlement_amount: Self::AssetType,
    ) -> Box<dyn Future<Item = (), Error = ()> + Send>;

    /// Clears the leftover data in the database and returns the cleared value
    fn load_uncredited_settlement_amount(
        &self,
        account_id: String,
    ) -> Box<dyn Future<Item = Self::AssetType, Error = ()> + Send>;
}
