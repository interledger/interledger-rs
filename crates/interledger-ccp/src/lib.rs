#[macro_use]
extern crate log;
#[macro_use]
extern crate lazy_static;

#[cfg(test)]
mod fixtures;
mod packet;
mod server;

pub trait RoutingAccount {
    fn should_send_routes(&self) -> bool;
    fn should_receive_routes(&self) -> bool;
}
