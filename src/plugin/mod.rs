use futures::{Sink, Stream};
use ilp::IlpPacket;

pub mod btp;
#[cfg(test)]
pub mod mock;

pub type IlpRequest = (u32, IlpPacket);
pub type PluginStream = Stream<Item = IlpRequest, Error = ()>;
pub type PluginSink = Sink<SinkItem = IlpRequest, SinkError = ()>;
pub trait Plugin:
    Stream<Item = IlpRequest, Error = ()>
    + Sink<SinkItem = IlpRequest, SinkError = ()>
    + Sized
    + Send
    + Sync
{
}
