#[macro_use]
extern crate log;

use bytes::Bytes;
use hashbrown::HashMap;
use interledger_service::{Account, AccountStore};

mod router;

pub use self::router::Router;

pub trait RouterStore: AccountStore + Clone + Send + Sync + 'static {
    // TODO avoid using HashMap because it means it'll be cloned a lot
    fn routing_table(&self) -> HashMap<Bytes, <Self::Account as Account>::AccountId>;
}
