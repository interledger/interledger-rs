#[macro_use]
extern crate log;

use bytes::Bytes;
use interledger_service::{Account, AccountStore};

mod router;

pub use self::router::Router;

pub trait RouterStore: AccountStore + Clone + Send + Sync + 'static {
    // TODO avoid using Vec because it means it'll be cloned a lot
    fn routing_table(&self) -> Vec<(Bytes, <Self::Account as Account>::AccountId)>;
}
