#[macro_use]
extern crate log;

use interledger_service::Account;

mod router;

pub use self::router::Router;

pub trait RouterStore: Clone + Send + Sync + 'static {
    type Account: Account;

    // TODO should this return a rayon::ParallelIterator?
    fn routing_table<'a>(&'a self) -> Box<Iterator<Item = (&'a [u8], &'a Self::Account)> + 'a>;
}
