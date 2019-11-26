use super::congestion::CongestionController;
use super::crypto::*;
use super::error::Error;
use super::packet::*;
use bytes::Bytes;
use bytes::BytesMut;
use futures::{Async, Future, Poll};
use interledger_ildcp::get_ildcp_info;
use interledger_packet::{
    Address, ErrorClass, ErrorCode as IlpErrorCode, Fulfill, PacketType as IlpPacketType,
    PrepareBuilder, Reject,
};
use interledger_service::*;
use log::{debug, error, warn};
use serde::{Deserialize, Serialize};
use std::{
    cell::Cell,
    cmp::min,
    str,
    time::{Duration, Instant, SystemTime},
};

// Maximum time we should wait since last fulfill before we error out to avoid
// getting into an infinite loop of sending packets and effectively DoSing ourselves
const MAX_TIME_SINCE_LAST_FULFILL: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Serialize, Deserialize, Eq, PartialEq)]
pub struct StreamDelivery {
    pub from: Address,
    pub to: Address,
    // StreamDelivery variables which we know ahead of time
    pub sent_amount: u64,
    pub sent_asset_scale: u8,
    pub sent_asset_code: String,
    pub delivered_amount: u64,
    // StreamDelivery variables which may get updated if the receiver sends us a
    // ConnectionAssetDetails frame.
    pub delivered_asset_scale: Option<u8>,
    pub delivered_asset_code: Option<String>,
}

impl StreamDelivery {
    fn increment_delivered_amount(&mut self, amount: u64) {
        self.delivered_amount += amount;
    }
}

/// Send a given amount of money using the STREAM transport protocol.
///
/// This returns the amount delivered, as reported by the receiver and in the receiver's asset's units.
pub fn send_money<S, A>(
    service: S,
    from_account: &A,
    destination_account: Address,
    shared_secret: &[u8],
    source_amount: u64,
) -> impl Future<Item = (StreamDelivery, S), Error = Error>
where
    S: IncomingService<A> + Clone,
    A: Account,
{
    let shared_secret = Bytes::from(shared_secret);
    let from_account = from_account.clone();
    // TODO can/should we avoid cloning the account?
    get_ildcp_info(&mut service.clone(), from_account.clone())
        .map_err(|_err| Error::ConnectionError("Unable to get ILDCP info: {:?}".to_string()))
        .and_then(move |account_details| {
            let source_account = account_details.ilp_address();
            if source_account.scheme() != destination_account.scheme() {
                warn!("Destination ILP address starts with a different scheme prefix (\"{}\') than ours (\"{}\'), this probably isn't going to work",
                destination_account.scheme(),
                source_account.scheme());
            }

            SendMoneyFuture {
                state: SendMoneyFutureState::SendMoney,
                next: Some(service),
                from_account: from_account.clone(),
                source_account,
                destination_account: destination_account.clone(),
                shared_secret,
                source_amount,
                // Try sending the full amount first
                // TODO make this configurable -- in different scenarios you might prioritize
                // sending as much as possible per packet vs getting money flowing ASAP differently
                congestion_controller: CongestionController::new(source_amount, source_amount / 10, 2.0),
                pending_requests: Cell::new(Vec::new()),
                receipt: StreamDelivery {
                    from: from_account.ilp_address().clone(),
                    to: destination_account,
                    sent_amount: source_amount,
                    sent_asset_scale: from_account.asset_scale(),
                    sent_asset_code: from_account.asset_code().to_string(),
                    delivered_asset_scale: None,
                    delivered_asset_code: None,
                    delivered_amount: 0,
                },
                should_send_source_account: true,
                sequence: 1,
                rejected_packets: 0,
                error: None,
                last_fulfill_time: Instant::now(),
            }
        })
}

struct SendMoneyFuture<S: IncomingService<A>, A: Account> {
    state: SendMoneyFutureState,
    next: Option<S>,
    from_account: A,
    source_account: Address,
    destination_account: Address,
    shared_secret: Bytes,
    source_amount: u64,
    congestion_controller: CongestionController,
    pending_requests: Cell<Vec<PendingRequest>>,
    receipt: StreamDelivery,
    should_send_source_account: bool,
    sequence: u64,
    rejected_packets: u64,
    error: Option<Error>,
    last_fulfill_time: Instant,
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
    // RemoteClosed,
    Closed,
}

impl<S, A> SendMoneyFuture<S, A>
where
    S: IncomingService<A>,
    A: Account,
{
    fn try_send_money(&mut self) -> Result<bool, Error> {
        // Fire off requests until the congestion controller tells us to stop or we've sent the total amount or maximum time since last fulfill has elapsed
        let mut sent_packets = false;
        loop {
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
                    source_account: self.source_account.clone(),
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
                destination: self.destination_account.clone(),
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
                sent_packets = true;
            } else {
                panic!("Polled after finish");
            }
        }
        Ok(sent_packets)
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
            destination: self.destination_account.clone(),
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

        if let Some(error) = self.error.take() {
            error!("Send money stopped because of error: {:?}", error);
            Err(error)
        } else if self.pending_requests.get_mut().is_empty() {
            Ok(Async::Ready(()))
        } else {
            Ok(Async::NotReady)
        }
    }

    fn handle_fulfill(&mut self, sequence: u64, amount: u64, fulfill: Fulfill) {
        // TODO should we check the fulfillment and expiry or can we assume the plugin does that?
        self.congestion_controller.fulfill(amount);
        self.should_send_source_account = false;
        self.last_fulfill_time = Instant::now();

        if let Ok(packet) = StreamPacket::from_encrypted(&self.shared_secret, fulfill.into_data()) {
            if packet.ilp_packet_type() == IlpPacketType::Fulfill {
                // TODO check that the sequence matches our outgoing packet

                // Update the asset scale & asset code via the received
                // frame. https://github.com/interledger/rfcs/pull/551
                // ensures that this won't change, so we only need to
                // perform this loop once.
                if self.receipt.delivered_asset_scale.is_none() {
                    for frame in packet.frames() {
                        if let Frame::ConnectionAssetDetails(frame) = frame {
                            self.receipt.delivered_asset_scale = Some(frame.source_asset_scale);
                            self.receipt.delivered_asset_code =
                                Some(frame.source_asset_code.to_string());
                        }
                    }
                }
                self.receipt
                    .increment_delivered_amount(packet.prepare_amount());
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
        self.rejected_packets += 1;
        debug!(
            "Prepare {} with amount {} was rejected with code: {} ({} left to send)",
            sequence,
            amount,
            reject.code(),
            self.source_amount
        );

        // if we receive a reject, try to update our asset code/scale
        // if it was not populated before
        if self.receipt.delivered_asset_scale.is_none()
            || self.receipt.delivered_asset_code.is_none()
        {
            if let Ok(packet) =
                StreamPacket::from_encrypted(&self.shared_secret, BytesMut::from(reject.data()))
            {
                for frame in packet.frames() {
                    if let Frame::ConnectionAssetDetails(frame) = frame {
                        self.receipt.delivered_asset_scale = Some(frame.source_asset_scale);
                        self.receipt.delivered_asset_code =
                            Some(frame.source_asset_code.to_string());
                    }
                }
            } else {
                warn!(
                    "Unable to parse STREAM packet from reject data for sequence {}",
                    sequence
                );
            }
        }

        match (reject.code().class(), reject.code()) {
            (ErrorClass::Temporary, _) => {}
            (_, IlpErrorCode::F08_AMOUNT_TOO_LARGE) => {
                // Handled by the congestion controller
            }
            (_, IlpErrorCode::F99_APPLICATION_ERROR) => {
                // TODO handle STREAM errors
            }
            _ => {
                self.error = Some(Error::SendMoneyError(format!(
                    "Packet was rejected with error: {} {}",
                    reject.code(),
                    str::from_utf8(reject.message()).unwrap_or_default(),
                )));
            }
        }
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
    type Item = (StreamDelivery, S);
    type Error = Error;

    fn poll(&mut self) -> Poll<Self::Item, Self::Error> {
        // TODO maybe don't have loops here and in try_send_money
        loop {
            self.poll_pending_requests()?;
            if self.last_fulfill_time.elapsed() >= MAX_TIME_SINCE_LAST_FULFILL {
                return Err(Error::TimeoutError(format!(
                    "Time since last fulfill exceeded the maximum time limit of {:?} secs",
                    self.last_fulfill_time.elapsed().as_secs()
                )));
            }

            if self.source_amount == 0 && self.pending_requests.get_mut().is_empty() {
                if self.state == SendMoneyFutureState::SendMoney {
                    self.state = SendMoneyFutureState::Closing;
                    self.try_send_connection_close()?;
                } else {
                    self.state = SendMoneyFutureState::Closed;
                    debug!(
                        "Send money future finished. Delivered: {} ({} packets fulfilled, {} packets rejected)", self.receipt.delivered_amount, self.sequence - 1, self.rejected_packets,
                    );
                    return Ok(Async::Ready((
                        self.receipt.clone(),
                        self.next.take().unwrap(),
                    )));
                }
            } else if !self.try_send_money()? {
                return Ok(Async::NotReady);
            }
        }
    }
}

#[cfg(test)]
mod send_money_tests {
    use super::*;
    use crate::test_helpers::{TestAccount, EXAMPLE_CONNECTOR};
    use interledger_ildcp::IldcpService;
    use interledger_packet::{ErrorCode as IlpErrorCode, RejectBuilder};
    use interledger_service::incoming_service_fn;
    use parking_lot::Mutex;
    use std::str::FromStr;
    use std::sync::Arc;
    use uuid::Uuid;

    #[test]
    fn stops_at_final_errors() {
        let account = TestAccount {
            id: Uuid::new_v4(),
            asset_code: "XYZ".to_string(),
            asset_scale: 9,
            ilp_address: Address::from_str("example.destination").unwrap(),
        };
        let requests = Arc::new(Mutex::new(Vec::new()));
        let requests_clone = requests.clone();
        let result = send_money(
            IldcpService::new(incoming_service_fn(move |request| {
                requests_clone.lock().push(request);
                Err(RejectBuilder {
                    code: IlpErrorCode::F00_BAD_REQUEST,
                    message: b"just some final error",
                    triggered_by: Some(&EXAMPLE_CONNECTOR),
                    data: &[],
                }
                .build())
            })),
            &account,
            Address::from_str("example.destination").unwrap(),
            &[0; 32][..],
            100,
        )
        .wait();
        assert!(result.is_err());
        assert_eq!(requests.lock().len(), 1);
    }
}
