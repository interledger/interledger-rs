use super::congestion::CongestionController;
use super::crypto::{
    fulfillment_to_condition, generate_condition, generate_fulfillment, random_condition,
    random_u32,
};
use super::data_money_stream::DataMoneyStream;
use super::packet::*;
use super::StreamPacket;
use bytes::{Bytes, BytesMut};
use chrono::{Duration, Utc};
use futures::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use futures::task;
use futures::task::Task;
use futures::{Async, Future, Poll, Stream};
use hex;
use ilp::{parse_f08_error, IlpFulfill, IlpPacket, IlpPrepare, IlpReject, PacketType};
use num_bigint::BigUint;
use num_traits::ToPrimitive;
use parking_lot::{Mutex, RwLock};
use plugin::IlpRequest;
use std::cmp::min;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

pub struct CloseFuture {
    conn: Connection,
}

impl Future for CloseFuture {
    type Item = ();
    type Error = ();

    fn poll(&mut self) -> Poll<Self::Item, Self::Error> {
        trace!("Polling to see whether connection was closed");
        self.conn.try_handle_incoming()?;

        if self.conn.state.load(Ordering::SeqCst) == ConnectionState::Closed as usize {
            trace!("Connection was closed, resolving close future");
            Ok(Async::Ready(()))
        } else {
            self.conn.try_send()?;
            trace!("Connection wasn't closed yet, returning NotReady");
            Ok(Async::NotReady)
        }
    }
}

#[derive(Clone)]
pub struct Connection {
    // TODO is it okay for this to be an AtomicUsize instead of a RwLock around the enum?
    // it ran into deadlocks with the RwLock
    state: Arc<AtomicUsize>,
    // TODO should this be a bounded sender? Don't want too many outgoing packets in the queue
    outgoing: UnboundedSender<IlpRequest>,
    incoming: Arc<Mutex<UnboundedReceiver<IlpRequest>>>,
    shared_secret: Bytes,
    source_account: Arc<String>,
    destination_account: Arc<String>,
    next_stream_id: Arc<AtomicUsize>,
    next_packet_sequence: Arc<AtomicUsize>,
    streams: Arc<RwLock<HashMap<u64, DataMoneyStream>>>,
    closed_streams: Arc<RwLock<HashSet<u64>>>,
    pending_outgoing_packets: Arc<Mutex<HashMap<u32, OutgoingPacketRecord>>>,
    new_streams: Arc<Mutex<VecDeque<u64>>>,
    frames_to_resend: Arc<Mutex<Vec<Frame>>>,
    // This is used to wake the task polling for incoming streams
    recv_task: Arc<Mutex<Option<Task>>>,
    // TODO add connection-level stats
    congestion_controller: Arc<Mutex<CongestionController>>,
}

struct OutgoingPacketRecord {
    original_amount: u64,
    original_packet: StreamPacket,
}

#[derive(PartialEq, Eq, Debug)]
#[repr(usize)]
enum ConnectionState {
    Opening,
    Open,
    Closing,
    CloseSent,
    Closed,
}

impl Connection {
    pub fn new(
        outgoing: UnboundedSender<IlpRequest>,
        incoming: UnboundedReceiver<IlpRequest>,
        shared_secret: Bytes,
        source_account: String,
        destination_account: String,
        is_server: bool,
    ) -> Self {
        let next_stream_id = if is_server { 2 } else { 1 };

        let conn = Connection {
            state: Arc::new(AtomicUsize::new(ConnectionState::Opening as usize)),
            outgoing,
            incoming: Arc::new(Mutex::new(incoming)),
            shared_secret,
            source_account: Arc::new(source_account),
            destination_account: Arc::new(destination_account),
            next_stream_id: Arc::new(AtomicUsize::new(next_stream_id)),
            next_packet_sequence: Arc::new(AtomicUsize::new(1)),
            streams: Arc::new(RwLock::new(HashMap::new())),
            closed_streams: Arc::new(RwLock::new(HashSet::new())),
            pending_outgoing_packets: Arc::new(Mutex::new(HashMap::new())),
            new_streams: Arc::new(Mutex::new(VecDeque::new())),
            frames_to_resend: Arc::new(Mutex::new(Vec::new())),
            recv_task: Arc::new(Mutex::new(None)),
            congestion_controller: Arc::new(Mutex::new(CongestionController::default())),
        };

        // TODO figure out a better way to send the initial packet - get the exchange rate and wait for response
        if !is_server {
            conn.send_handshake();
        }

        // TODO wait for handshake reply before setting state to open
        conn.state
            .store(ConnectionState::Open as usize, Ordering::SeqCst);

        conn
    }

    pub fn create_stream(&self) -> DataMoneyStream {
        let id = self.next_stream_id.fetch_add(2, Ordering::SeqCst) as u64;
        let stream = DataMoneyStream::new(id, self.clone());
        (*self.streams.write()).insert(id, stream.clone());
        debug!("Created stream {}", id);
        stream
    }

    pub fn close(&self) -> CloseFuture {
        debug!("Closing connection");
        self.state
            .store(ConnectionState::Closing as usize, Ordering::SeqCst);
        // TODO make sure we don't send stream close frames for every stream
        for stream in (*self.streams.read()).values() {
            stream.set_closing();
        }

        CloseFuture { conn: self.clone() }
    }

    pub(super) fn is_closed(&self) -> bool {
        self.state.load(Ordering::SeqCst) == ConnectionState::Closed as usize
    }

    pub(super) fn try_send(&self) -> Result<(), ()> {
        // Loop until we don't need to send any more packets or doing so would
        // violate limits imposed by the congestion controller
        loop {
            if self.is_closed() {
                trace!("Connection was closed, not sending any more packets");
                return Ok(());
            }

            trace!("Checking if we should send an outgoing packet");

            let mut outgoing_amount: u64 = 0;
            let mut frames: Vec<Frame> = Vec::new();
            let mut closed_streams: Vec<u64> = Vec::new();

            let mut congestion_controller = self.congestion_controller.lock();
            let max_packet_amount = congestion_controller.get_max_amount();

            for stream in (*self.streams.read()).values() {
                // Send money
                if max_packet_amount > 0 {
                    trace!("Checking if stream {} has money or data to send", stream.id);
                    let stream_amount = stream.money.send_max()
                        - stream.money.pending()
                        - stream.money.total_sent();
                    let amount_to_send = min(stream_amount, max_packet_amount - outgoing_amount);
                    if amount_to_send > 0 {
                        trace!("Stream {} sending {}", stream.id, amount_to_send);
                        stream.money.add_to_pending(amount_to_send);
                        outgoing_amount += amount_to_send;
                        frames.push(Frame::StreamMoney(StreamMoneyFrame {
                            stream_id: BigUint::from(stream.id),
                            shares: BigUint::from(amount_to_send),
                        }));
                    }
                }

                // Send data
                // TODO don't send too much data
                let max_data: usize = 1_000_000_000;
                if let Some((data, offset)) = stream.data.get_outgoing_data(max_data) {
                    trace!(
                        "Stream {} has {} bytes to send (offset: {})",
                        stream.id,
                        data.len(),
                        offset
                    );
                    frames.push(Frame::StreamData(StreamDataFrame {
                        stream_id: BigUint::from(stream.id),
                        data,
                        offset: BigUint::from(offset),
                    }))
                } else {
                    trace!("Stream {} does not have any data to send", stream.id);
                }

                // Inform other side about closing streams
                if stream.is_closing() {
                    trace!("Sending stream close frame for stream {}", stream.id);
                    frames.push(Frame::StreamClose(StreamCloseFrame {
                        stream_id: BigUint::from(stream.id),
                        code: ErrorCode::NoError,
                        message: String::new(),
                    }));
                    closed_streams.push(stream.id);
                    stream.set_closed();
                    // TODO don't block
                    (*self.closed_streams.write()).insert(stream.id);
                }
            }

            if self.state.load(Ordering::SeqCst) == ConnectionState::Closing as usize {
                trace!("Sending connection close frame");
                frames.push(Frame::ConnectionClose(ConnectionCloseFrame {
                    code: ErrorCode::NoError,
                    message: String::new(),
                }));
                self.state
                    .store(ConnectionState::CloseSent as usize, Ordering::SeqCst);
            }

            if frames.is_empty() {
                trace!("Not sending packet, no frames need to be sent");
                return Ok(());
            }

            // Note we need to remove them after we've given up the read lock on self.streams
            if !closed_streams.is_empty() {
                let mut streams = self.streams.write();
                for stream_id in closed_streams.iter() {
                    debug!("Removed stream {}", stream_id);
                    streams.remove(&stream_id);
                }
            }

            let stream_packet = StreamPacket {
                sequence: self.next_packet_sequence.fetch_add(1, Ordering::SeqCst) as u64,
                ilp_packet_type: PacketType::IlpPrepare,
                prepare_amount: 0, // TODO set min amount
                frames,
            };

            let encrypted = stream_packet.to_encrypted(&self.shared_secret);
            let condition = generate_condition(&self.shared_secret, &encrypted);
            let prepare = IlpPrepare::new(
                self.destination_account.to_string(),
                outgoing_amount,
                condition,
                // TODO use less predictable timeout
                Utc::now() + Duration::seconds(30),
                encrypted,
            );
            let request_id = random_u32();
            let request = (request_id, IlpPacket::Prepare(prepare));
            debug!(
                "Sending outgoing request {} with stream packet: {:?}",
                request_id, stream_packet
            );

            congestion_controller.prepare(request_id, outgoing_amount);

            (*self.pending_outgoing_packets.lock()).insert(
                request_id,
                OutgoingPacketRecord {
                    original_amount: outgoing_amount,
                    original_packet: stream_packet.clone(),
                },
            );

            self.outgoing.unbounded_send(request).map_err(|err| {
                error!("Error sending outgoing packet: {:?}", err);
            })?;
        }
    }

    pub(super) fn try_handle_incoming(&self) -> Result<(), ()> {
        // Handle incoming requests until there are no more
        // Note: looping until we get Async::NotReady tells Tokio to wake us up when there are more incoming requests
        loop {
            if self.is_closed() {
                trace!("Connection was closed, not handling any more incoming packets");
                return Ok(());
            }

            trace!("Polling for incoming requests");
            let next = (*self.incoming.lock()).poll();

            match next {
                Ok(Async::Ready(Some((request_id, packet)))) => {
                    match packet {
                        IlpPacket::Prepare(prepare) => {
                            self.handle_incoming_prepare(request_id, prepare)?
                        }
                        IlpPacket::Fulfill(fulfill) => self.handle_fulfill(request_id, fulfill)?,
                        IlpPacket::Reject(reject) => self.handle_reject(request_id, reject)?,
                    }
                    self.try_send()?
                }
                Ok(Async::Ready(None)) => {
                    error!("Incoming stream closed");
                    // TODO should this error?
                    return Ok(());
                }
                Ok(Async::NotReady) => {
                    trace!("No more incoming requests for now");
                    return Ok(());
                }
                Err(err) => {
                    error!("Error polling incoming request stream: {:?}", err);
                    return Err(());
                }
            };
        }
    }

    fn handle_incoming_prepare(&self, request_id: u32, prepare: IlpPrepare) -> Result<(), ()> {
        debug!("Handling incoming prepare {}", request_id);

        let response_frames: Vec<Frame> = Vec::new();

        let fulfillment = generate_fulfillment(&self.shared_secret, &prepare.data);
        let condition = fulfillment_to_condition(&fulfillment);
        let is_fulfillable = condition == prepare.execution_condition;

        // TODO avoid copying data
        let stream_packet =
            StreamPacket::from_encrypted(&self.shared_secret, BytesMut::from(prepare.data));
        if stream_packet.is_err() {
            warn!(
                "Got Prepare with data that we cannot parse. Rejecting request {}",
                request_id
            );
            self.outgoing
                .unbounded_send((
                    request_id,
                    IlpPacket::Reject(IlpReject::new("F02", "", "", Bytes::new())),
                ))
                .map_err(|err| {
                    error!("Error sending Reject {} {:?}", request_id, err);
                })?;
            return Ok(());
        }
        let stream_packet = stream_packet.unwrap();

        debug!(
            "Prepare {} had stream packet: {:?}",
            request_id, stream_packet
        );

        // Handle new streams
        for frame in stream_packet.frames.iter() {
            match frame {
                Frame::StreamMoney(frame) => {
                    self.handle_new_stream(frame.stream_id.to_u64().unwrap());
                }
                Frame::StreamData(frame) => {
                    self.handle_new_stream(frame.stream_id.to_u64().unwrap());
                }
                // TODO handle other frames that open streams
                _ => {}
            }
        }

        // Count up the total number of money "shares" in the packet
        let total_money_shares: u64 = stream_packet.frames.iter().fold(0, |sum, frame| {
            if let Frame::StreamMoney(frame) = frame {
                sum + frame.shares.to_u64().unwrap()
            } else {
                sum
            }
        });

        // Handle incoming money
        if is_fulfillable {
            for frame in stream_packet.frames.iter() {
                if let Frame::StreamMoney(frame) = frame {
                    // TODO only add money to incoming if sending the fulfill is successful
                    // TODO make sure all other checks pass first
                    let stream_id = frame.stream_id.to_u64().unwrap();
                    let streams = self.streams.read();
                    let stream = streams.get(&stream_id).unwrap();
                    let amount: u64 =
                        frame.shares.to_u64().unwrap() * prepare.amount / total_money_shares;
                    debug!("Stream {} received {}", stream_id, amount);
                    stream.money.add_received(amount);
                    stream.money.try_wake_polling();
                }
            }
        }

        self.handle_incoming_data(&stream_packet)?;

        self.handle_stream_closes(&stream_packet);

        self.handle_connection_close(&stream_packet);

        // Fulfill or reject Preapre
        if is_fulfillable {
            let response_packet = StreamPacket {
                sequence: stream_packet.sequence,
                ilp_packet_type: PacketType::IlpFulfill,
                prepare_amount: prepare.amount,
                frames: response_frames,
            };
            let encrypted_response = response_packet.to_encrypted(&self.shared_secret);
            let fulfill =
                IlpPacket::Fulfill(IlpFulfill::new(fulfillment.clone(), encrypted_response));
            debug!(
                "Fulfilling request {} with fulfillment: {} and encrypted stream packet: {:?}",
                request_id,
                hex::encode(&fulfillment[..]),
                response_packet
            );
            self.outgoing.unbounded_send((request_id, fulfill)).unwrap();
        } else {
            let response_packet = StreamPacket {
                sequence: stream_packet.sequence,
                ilp_packet_type: PacketType::IlpReject,
                prepare_amount: prepare.amount,
                frames: response_frames,
            };
            let encrypted_response = response_packet.to_encrypted(&self.shared_secret);
            let reject = IlpPacket::Reject(IlpReject::new("F99", "", "", encrypted_response));
            debug!(
                "Rejecting request {} and including encrypted stream packet {:?}",
                request_id, response_packet
            );
            self.outgoing.unbounded_send((request_id, reject)).unwrap();
        }

        Ok(())
    }

    fn handle_new_stream(&self, stream_id: u64) {
        // TODO make sure they don't open streams with our number (even or odd, depending on whether we're the client or server)
        let is_new = !(*self.streams.read()).contains_key(&stream_id);
        let already_closed = (*self.closed_streams.read()).contains(&stream_id);
        if is_new && !already_closed {
            debug!("Got new stream {}", stream_id);
            let stream = DataMoneyStream::new(stream_id, self.clone());
            (*self.streams.write()).insert(stream_id, stream);
            (*self.new_streams.lock()).push_back(stream_id);
        }
    }

    fn handle_incoming_data(&self, stream_packet: &StreamPacket) -> Result<(), ()> {
        for frame in stream_packet.frames.iter() {
            if let Frame::StreamData(frame) = frame {
                let stream_id = frame.stream_id.to_u64().unwrap();
                let streams = self.streams.read();
                let stream = streams.get(&stream_id).unwrap();
                // TODO make sure the offset number isn't too big
                let data = frame.data.clone();
                let offset = frame.offset.to_usize().unwrap();
                debug!(
                    "Stream {} got {} bytes of incoming data",
                    stream.id,
                    data.len()
                );
                stream.data.push_incoming_data(data, offset)?;
                stream.data.try_wake_polling();
            }
        }
        Ok(())
    }

    fn handle_stream_closes(&self, stream_packet: &StreamPacket) {
        for frame in stream_packet.frames.iter() {
            if let Frame::StreamClose(frame) = frame {
                let stream_id = frame.stream_id.to_u64().unwrap();
                debug!("Remote closed stream {}", stream_id);
                let streams = self.streams.read();
                let stream = streams.get(&stream_id).unwrap();
                // TODO finish sending the money and data first
                stream.set_closed();
            }
        }
    }

    fn handle_connection_close(&self, stream_packet: &StreamPacket) {
        for frame in stream_packet.frames.iter() {
            if let Frame::ConnectionClose(frame) = frame {
                debug!(
                    "Remote closed connection with code: {:?}: {}",
                    frame.code, frame.message
                );
                self.close_now();
            }
        }
    }

    fn close_now(&self) {
        debug!("Closing connection now");
        self.state
            .store(ConnectionState::Closed as usize, Ordering::SeqCst);

        for stream in (*self.streams.read()).values() {
            stream.set_closed();
        }

        (*self.incoming.lock()).close();

        // Wake up the task polling for incoming streams so it ends
        self.try_wake_polling();
    }

    fn handle_fulfill(&self, request_id: u32, fulfill: IlpFulfill) -> Result<(), ()> {
        debug!(
            "Request {} was fulfilled with fulfillment: {}",
            request_id,
            hex::encode(&fulfill.fulfillment[..])
        );

        let OutgoingPacketRecord {
            original_amount,
            original_packet,
        } = (*self.pending_outgoing_packets.lock())
            .remove(&request_id)
            .unwrap();

        let response = {
            let decrypted =
                StreamPacket::from_encrypted(&self.shared_secret, BytesMut::from(fulfill.data))
                    .ok();
            if let Some(packet) = decrypted {
                if packet.sequence != original_packet.sequence {
                    warn!("Got Fulfill with stream packet whose sequence does not match the original request. Request ID: {}, sequence: {}, fulfill packet: {:?}", request_id, original_packet.sequence, packet);
                    None
                } else if packet.ilp_packet_type != PacketType::IlpFulfill {
                    warn!("Got Fulfill with stream packet that should have been on a differen type of ILP packet. Request ID: {}, fulfill packet: {:?}", request_id, packet);
                    None
                } else {
                    trace!("Got Fulfill with stream packet: {:?}", packet);
                    Some(packet)
                }
            } else {
                None
            }
        };

        (*self.congestion_controller.lock()).fulfill(request_id);

        let total_delivered = {
            match response.as_ref() {
                Some(packet) => packet.prepare_amount,
                None => 0,
            }
        };

        for frame in original_packet.frames.iter() {
            if let Frame::StreamMoney(frame) = frame {
                let stream_id = frame.stream_id.to_u64().unwrap();
                let streams = self.streams.read();
                let stream = streams.get(&stream_id).unwrap();

                let shares = frame.shares.to_u64().unwrap();
                stream.money.pending_to_sent(shares);

                let amount_delivered: u64 = total_delivered * shares / original_amount;
                stream.money.add_delivered(amount_delivered);
            }
        }

        if let Some(packet) = response.as_ref() {
            self.handle_incoming_data(&packet)?;
        }

        // TODO handle response frames

        // Close the connection if they sent a close frame or we sent one and they ACKed it
        if let Some(packet) = response {
            self.handle_connection_close(&packet);
        }
        let we_sent_close_frame = original_packet.frames.iter().any(|frame| {
            if let Frame::ConnectionClose(_) = frame {
                true
            } else {
                false
            }
        });
        if we_sent_close_frame {
            debug!("ConnectionClose frame was ACKed, closing connection now");
            self.close_now();
        }

        Ok(())
    }

    fn handle_reject(&self, request_id: u32, reject: IlpReject) -> Result<(), ()> {
        debug!(
            "Request {} was rejected with code: {}",
            request_id, reject.code
        );

        let entry = (*self.pending_outgoing_packets.lock()).remove(&request_id);
        if entry.is_none() {
            return Ok(());
        }
        let OutgoingPacketRecord {
            original_amount,
            mut original_packet,
        } = entry.unwrap();

        // Handle F08 errors, which communicate the maximum packet amount
        if let Some(err_details) = parse_f08_error(&reject) {
            let max_packet_amount: u64 =
                original_amount * err_details.max_amount / err_details.amount_received;
            debug!("Found path Maximum Packet Amount: {}", max_packet_amount);
            (*self.congestion_controller.lock()).set_max_packet_amount(max_packet_amount);
        }

        // Parse STREAM response packet from F99 errors
        let response = {
            if &reject.code == "F99" && !reject.data.is_empty() {
                match StreamPacket::from_encrypted(&self.shared_secret, BytesMut::from(reject.data))
                {
                    Ok(packet) => {
                        if packet.sequence != original_packet.sequence {
                            warn!("Got Reject with stream packet whose sequence does not match the original request. Request ID: {}, sequence: {}, packet: {:?}", request_id, original_packet.sequence, packet);
                            None
                        } else if packet.ilp_packet_type != PacketType::IlpReject {
                            warn!("Got Reject with stream packet that should have been on a differen type of ILP packet. Request ID: {}, packet: {:?}", request_id, packet);
                            None
                        } else {
                            trace!("Got Reject with stream packet: {:?}", packet);
                            Some(packet)
                        }
                    }
                    Err(err) => {
                        warn!(
                            "Got encrypted response packet that we could not decrypt: {:?}",
                            err
                        );
                        None
                    }
                }
            } else {
                None
            }
        };

        (*self.congestion_controller.lock()).reject(request_id, &reject.code);

        // Release pending money
        for frame in original_packet.frames.iter() {
            if let Frame::StreamMoney(frame) = frame {
                let stream_id = frame.stream_id.to_u64().unwrap();
                let streams = self.streams.read();
                let stream = streams.get(&stream_id).unwrap();

                let shares = frame.shares.to_u64().unwrap();
                stream.money.subtract_from_pending(shares);
            }
        }
        // TODO handle response frames

        if let Some(packet) = response.as_ref() {
            self.handle_incoming_data(&packet)?;

            self.handle_connection_close(&packet);
        }

        // Only resend frames if they didn't get to the receiver
        if response.is_none() {
            let mut frames_to_resend = self.frames_to_resend.lock();
            while !original_packet.frames.is_empty() {
                match original_packet.frames.pop().unwrap() {
                    Frame::StreamData(frame) => frames_to_resend.push(Frame::StreamData(frame)),
                    Frame::StreamClose(frame) => frames_to_resend.push(Frame::StreamClose(frame)),
                    Frame::ConnectionClose(frame) => {
                        frames_to_resend.push(Frame::ConnectionClose(frame))
                    }
                    _ => {}
                }
            }
        }

        Ok(())
    }

    fn send_handshake(&self) {
        let sequence = self.next_packet_sequence.fetch_add(1, Ordering::SeqCst) as u64;
        let packet = StreamPacket {
            sequence,
            ilp_packet_type: PacketType::IlpPrepare,
            prepare_amount: 0,
            frames: vec![Frame::ConnectionNewAddress(ConnectionNewAddressFrame {
                source_account: self.source_account.to_string(),
            })],
        };
        self.send_unfulfillable_prepare(&packet);
    }

    // TODO wait for response
    fn send_unfulfillable_prepare(&self, stream_packet: &StreamPacket) -> () {
        let request_id = random_u32();
        let prepare = IlpPacket::Prepare(IlpPrepare::new(
            // TODO do we need to clone this?
            self.destination_account.to_string(),
            0,
            random_condition(),
            Utc::now() + Duration::seconds(30),
            stream_packet.to_encrypted(&self.shared_secret),
        ));
        self.outgoing.unbounded_send((request_id, prepare)).unwrap();
    }

    fn try_wake_polling(&self) {
        if let Some(task) = (*self.recv_task.lock()).take() {
            debug!("Notifying incoming stream poller that it should wake up");
            task.notify();
        }
    }
}

impl Stream for Connection {
    type Item = DataMoneyStream;
    type Error = ();

    fn poll(&mut self) -> Poll<Option<Self::Item>, Self::Error> {
        trace!("Polling for new incoming streams");
        self.try_handle_incoming()?;

        // Store the current task so that it can be woken up if the
        // MoneyStream or DataStream poll for incoming packets and the connection is closed
        *self.recv_task.lock() = Some(task::current());

        let mut new_streams = self.new_streams.lock();
        if let Some(stream_id) = new_streams.pop_front() {
            let streams = self.streams.read();
            Ok(Async::Ready(Some(streams.get(&stream_id).unwrap().clone())))
        } else if self.is_closed() {
            trace!("Connection was closed, no more incoming streams");
            Ok(Async::Ready(None))
        } else {
            Ok(Async::NotReady)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::sync::mpsc::unbounded;

    fn test_conn() -> (
        Connection,
        UnboundedSender<IlpRequest>,
        UnboundedReceiver<IlpRequest>,
    ) {
        let (incoming_tx, incoming_rx) = unbounded::<IlpRequest>();
        let (outgoing_tx, outgoing_rx) = unbounded::<IlpRequest>();
        let conn = Connection::new(
            outgoing_tx,
            incoming_rx,
            Bytes::from(&[0u8; 32][..]),
            String::from("example.alice"),
            String::from("example.bob"),
            // Set as server so it doesn't bother sending the handshake
            true,
        );
        (conn, incoming_tx, outgoing_rx)
    }

    mod max_packet_amount {
        use super::*;
        use futures::future::ok;
        use futures::Sink;
        use ilp::packet::create_f08_error;
        use tokio::runtime::current_thread::block_on_all;

        #[test]
        fn discovery() {
            let (conn, incoming, outgoing) = test_conn();
            let mut stream = conn.create_stream();

            // Send money
            stream.money.start_send(100).unwrap();
            let (request, outgoing) = outgoing.into_future().wait().unwrap();
            let (request_id, _prepare) = request.unwrap();

            // Respond with F08
            let error = (request_id, IlpPacket::Reject(create_f08_error(100, 50)));
            incoming.unbounded_send(error).unwrap();
            block_on_all(ok(()).and_then(|_| conn.try_handle_incoming())).unwrap();

            // Check that next packet is smaller
            let (request, _outgoing) = outgoing.into_future().wait().unwrap();
            let (_request_id, prepare) = request.unwrap();

            if let IlpPacket::Prepare(prepare) = prepare {
                assert_eq!(prepare.amount, 50);
            } else {
                assert!(false);
            }
        }

        #[test]
        fn discovery_with_exchange_rate() {
            let (conn, incoming, outgoing) = test_conn();
            let mut stream = conn.create_stream();

            // Send money
            stream.money.start_send(1000).unwrap();
            let (request, outgoing) = outgoing.into_future().wait().unwrap();
            let (request_id, _prepare) = request.unwrap();

            // Respond with F08
            let error = (request_id, IlpPacket::Reject(create_f08_error(542, 107)));
            incoming.unbounded_send(error).unwrap();
            block_on_all(ok(()).and_then(|_| conn.try_handle_incoming())).unwrap();

            // Check that next packet is smaller
            let (request, _outgoing) = outgoing.into_future().wait().unwrap();
            let (_request_id, prepare) = request.unwrap();

            if let IlpPacket::Prepare(prepare) = prepare {
                assert_eq!(prepare.amount, 197);
            } else {
                assert!(false);
            }
        }
    }
}
