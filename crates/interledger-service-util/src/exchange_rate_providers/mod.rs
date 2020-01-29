/// Exchange rate provider for [CoinCap](https://coincap.io/)
mod coincap;
/// Exchange rate provider for [CryptoCompare](https://www.cryptocompare.com/). REQUIRES [API KEY](https://min-api.cryptocompare.com/).
mod cryptocompare;

pub use coincap::*;
pub use cryptocompare::*;
