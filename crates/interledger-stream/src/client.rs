use super::congestion::CongestionController;
use super::crypto::*;
use super::error::Error;
use super::packet::*;
use bytes::Bytes;
use futures::{Async, Future, Poll};
use interledger_ildcp::get_ildcp_info;
use interledger_packet::{Fulfill, PacketType as IlpPacketType, PrepareBuilder, Reject};
use interledger_service::*;
use std::cell::Cell;
use std::cmp::min;
use std::time::{Duration, SystemTime};

pub fn send_money<S, A>(
    service: S,
    from_account: &A,
    destination_account: &[u8],
    shared_secret: &[u8],
    source_amount: u64,
) -> impl Future<Item = (u64, S), Error = Error>
where
    S: IncomingService<A> + Clone,
    A: Account,
{
    let destination_account = Bytes::from(destination_account);
    let shared_secret = Bytes::from(shared_secret);
    let from_account = from_account.clone();
    // TODO can/should we avoid cloning the account?
    get_ildcp_info(&mut service.clone(), from_account.clone())
        .map_err(|_err| Error::ConnectionError("Unable to get ILDCP info: {:?}".to_string()))
        .and_then(move |account_details| SendMoneyFuture {
            state: SendMoneyFutureState::SendMoney,
            next: Some(service),
            from_account: from_account,
            source_account: Bytes::from(account_details.client_address()),
            destination_account,
            shared_secret,
            source_amount,
            congestion_controller: CongestionController::default(),
            pending_requests: Cell::new(Vec::new()),
            amount_delivered: 0,
            should_send_source_account: true,
            sequence: 1,
        })
}

struct SendMoneyFuture<S: IncomingService<A>, A: Account> {
    state: SendMoneyFutureState,
    next: Option<S>,
    from_account: A,
    source_account: Bytes,
    destination_account: Bytes,
    shared_secret: Bytes,
    source_amount: u64,
    congestion_controller: CongestionController,
    pending_requests: Cell<Vec<PendingRequest>>,
    amount_delivered: u64,
    should_send_source_account: bool,
    sequence: u64,
}

struct PendingRequest {
    sequence: u64,
    amount: u64,
    future: BoxedIlpFuture,
}

#[derive(PartialEq)]
enum SendMoneyFutureState {
    SendMoney,
    Closing,
    Closed,
}

impl<S, A> SendMoneyFuture<S, A>
where
    S: IncomingService<A>,
    A: Account,
{
    fn try_send_money(&mut self) -> Result<(), Error> {
        // Fire off requests until the congestion controller tells us to stop or we've sent the total amount
        loop {
            // Determine the amount to send
            let amount = min(
                self.source_amount,
                self.congestion_controller.get_max_amount(),
            );
            if amount == 0 {
                break;
            }
            self.source_amount -= amount;

            // Load up the STREAM packet
            let sequence = self.next_sequence();
            let mut frames = vec![Frame::StreamMoney(StreamMoneyFrame {
                stream_id: 1,
                shares: 1,
            })];
            if self.should_send_source_account {
                frames.push(Frame::ConnectionNewAddress(ConnectionNewAddressFrame {
                    source_account: &self.source_account[..],
                }));
            }
            let stream_packet = StreamPacketBuilder {
                ilp_packet_type: IlpPacketType::Prepare,
                // TODO enforce min exchange rate
                prepare_amount: 0,
                sequence,
                frames: &frames,
            }
            .build();

            // Create the ILP Prepare packet
            debug!(
                "Sending packet {} with amount: {} and encrypted STREAM packet: {:?}",
                sequence, amount, stream_packet
            );
            let data = stream_packet.into_encrypted(&self.shared_secret);
            let execution_condition = generate_condition(&self.shared_secret, &data);
            let prepare = PrepareBuilder {
                destination: &self.destination_account[..],
                amount,
                execution_condition: &execution_condition,
                expires_at: SystemTime::now() + Duration::from_secs(30),
                // TODO don't copy the data
                data: &data[..],
            }
            .build();

            // Send it!
            self.congestion_controller.prepare(amount);
            if let Some(ref mut next) = self.next {
                let send_request = next.handle_request(IncomingRequest {
                    from: self.from_account.clone(),
                    prepare,
                });
                self.pending_requests.get_mut().push(PendingRequest {
                    sequence,
                    amount,
                    future: Box::new(send_request),
                });
            } else {
                panic!("Polled after finish");
            }
        }
        self.poll_pending_requests()?;
        Ok(())
    }

    fn try_send_connection_close(&mut self) -> Result<(), Error> {
        let sequence = self.next_sequence();
        let stream_packet = StreamPacketBuilder {
            ilp_packet_type: IlpPacketType::Prepare,
            prepare_amount: 0,
            sequence,
            frames: &[Frame::ConnectionClose(ConnectionCloseFrame {
                code: ErrorCode::NoError,
                message: "",
            })],
        }
        .build();
        // Create the ILP Prepare packet
        let data = stream_packet.into_encrypted(&self.shared_secret);
        let prepare = PrepareBuilder {
            destination: &self.destination_account[..],
            amount: 0,
            execution_condition: &random_condition(),
            expires_at: SystemTime::now() + Duration::from_secs(30),
            data: &data[..],
        }
        .build();

        // Send it!
        debug!("Closing connection");
        if let Some(ref mut next) = self.next {
            let send_request = next.handle_request(IncomingRequest {
                from: self.from_account.clone(),
                prepare,
            });
            self.pending_requests.get_mut().push(PendingRequest {
                sequence,
                amount: 0,
                future: Box::new(send_request),
            });
        } else {
            panic!("Polled after finish");
        }
        self.poll_pending_requests()?;
        Ok(())
    }

    fn poll_pending_requests(&mut self) -> Poll<(), Error> {
        let pending_requests = self.pending_requests.take();
        let pending_requests = pending_requests
            .into_iter()
            .filter_map(|mut pending_request| match pending_request.future.poll() {
                Ok(Async::NotReady) => Some(pending_request),
                Ok(Async::Ready(fulfill)) => {
                    self.handle_fulfill(pending_request.sequence, pending_request.amount, fulfill);
                    None
                }
                Err(reject) => {
                    self.handle_reject(pending_request.sequence, pending_request.amount, reject);
                    None
                }
            })
            .collect();
        self.pending_requests.set(pending_requests);

        if self.pending_requests.get_mut().is_empty() {
            Ok(Async::Ready(()))
        } else {
            Ok(Async::NotReady)
        }
    }

    fn handle_fulfill(&mut self, sequence: u64, amount: u64, fulfill: Fulfill) {
        // TODO should we check the fulfillment and expiry or can we assume the plugin does that?
        self.congestion_controller.fulfill(amount);
        self.should_send_source_account = false;

        if let Ok(packet) = StreamPacket::from_encrypted(&self.shared_secret, fulfill.into_data()) {
            if packet.ilp_packet_type() == IlpPacketType::Fulfill {
                // TODO check that the sequence matches our outgoing packet
                self.amount_delivered += packet.prepare_amount();
            }
        } else {
            warn!(
                "Unable to parse STREAM packet from fulfill data for sequence {}",
                sequence
            );
        }

        debug!(
            "Prepare {} with amount {} was fulfilled ({} left to send)",
            sequence, amount, self.source_amount
        );
    }

    fn handle_reject(&mut self, sequence: u64, amount: u64, reject: Reject) {
        self.source_amount += amount;
        self.congestion_controller.reject(amount, &reject);
        debug!(
            "Prepare {} with amount {} was rejected with code: {} ({} left to send)",
            sequence,
            amount,
            reject.code(),
            self.source_amount
        );
    }

    fn next_sequence(&mut self) -> u64 {
        let seq = self.sequence;
        self.sequence += 1;
        seq
    }
}

impl<S, A> Future for SendMoneyFuture<S, A>
where
    S: IncomingService<A>,
    A: Account,
{
    type Item = (u64, S);
    type Error = Error;

    fn poll(&mut self) -> Poll<Self::Item, Self::Error> {
        // Try polling the pending requests
        self.poll_pending_requests()?;

        if self.source_amount == 0 && self.pending_requests.get_mut().is_empty() {
            if self.state == SendMoneyFutureState::SendMoney {
                self.try_send_connection_close()?;
                self.state = SendMoneyFutureState::Closing;
                Ok(Async::NotReady)
            } else {
                self.state = SendMoneyFutureState::Closed;
                Ok(Async::Ready((
                    self.amount_delivered,
                    self.next.take().unwrap(),
                )))
            }
        } else {
            self.try_send_money()?;
            Ok(Async::NotReady)
        }
    }
}
