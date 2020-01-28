use super::congestion::CongestionController;
use super::crypto::*;
use super::error::Error;
use super::packet::*;
use bytes::Bytes;
use bytes::BytesMut;
use futures::{ready, TryFutureExt};
use interledger_ildcp::get_ildcp_info;
use interledger_packet::{
    Address, ErrorClass, ErrorCode as IlpErrorCode, Fulfill, PacketType as IlpPacketType,
    PrepareBuilder, Reject,
};
use interledger_service::*;
use log::{debug, error, warn};
use pin_project::{pin_project, project};
use serde::{Deserialize, Serialize};
use std::{
    cell::Cell,
    cmp::min,
    str,
    time::{Duration, Instant, SystemTime},
};
use std::{
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};

/// Maximum time we should wait since last fulfill before we error out to avoid
/// getting into an infinite loop of sending packets and effectively DoSing ourselves
const MAX_TIME_SINCE_LAST_FULFILL: Duration = Duration::from_secs(30);

/// Metadata about a completed STREAM payment
#[derive(Debug, Clone, Serialize, Deserialize, Eq, PartialEq)]
pub struct StreamDelivery {
    /// The sender's ILP Address
    pub from: Address,
    /// The receiver's ILP Address
    pub to: Address,
    // StreamDelivery variables which we know ahead of time
    /// The amount sent by the sender
    pub sent_amount: u64,
    /// The sender's asset scale
    pub sent_asset_scale: u8,
    /// The sender's asset code
    pub sent_asset_code: String,
    /// The amount delivered to the receiver
    pub delivered_amount: u64,
    // StreamDelivery variables which may get updated if the receiver sends us a
    // ConnectionAssetDetails frame.
    /// The asset scale delivered to the receiver
    /// (this may change depending on the granularity of accounts across nodes)
    pub delivered_asset_scale: Option<u8>,
    /// The asset code delivered to the receiver (this may happen in cross-currency payments)
    pub delivered_asset_code: Option<String>,
}

impl StreamDelivery {
    /// Increases the `StreamDelivery`'s [`delivered_amount`](./struct.StreamDelivery.html#structfield.delivered_amount) by `amount`
    fn increment_delivered_amount(&mut self, amount: u64) {
        self.delivered_amount += amount;
    }
}

/// Send a given amount of money using the STREAM transport protocol.
///
/// This returns the amount delivered, as reported by the receiver and in the receiver's asset's units.
pub async fn send_money<S, A>(
    service: S,
    from_account: &A,
    destination_account: Address,
    shared_secret: &[u8],
    source_amount: u64,
) -> Result<(StreamDelivery, S), Error>
where
    S: IncomingService<A> + Send + Sync + Clone + 'static,
    A: Account + Send + Sync + 'static,
{
    let shared_secret = Bytes::from(shared_secret);
    let from_account = from_account.clone();
    // TODO can/should we avoid cloning the account?
    let account_details = get_ildcp_info(&mut service.clone(), from_account.clone())
        .map_err(|_err| Error::ConnectionError("Unable to get ILDCP info: {:?}".to_string()))
        .await?;

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
    .await
}

#[pin_project]
/// Helper data type used to track a streaming payment
struct SendMoneyFuture<S: IncomingService<A>, A: Account> {
    /// The future's [state](./enum.SendMoneyFutureState.html)
    state: SendMoneyFutureState,
    /// The next service which will receive the Stream packet
    next: Option<S>,
    /// The account sending the STREAM payment
    from_account: A,
    /// The ILP Address of the account sending the payment
    source_account: Address,
    /// The ILP Address of the account receiving the payment
    destination_account: Address,
    /// The shared secret generated by the sender and the receiver
    shared_secret: Bytes,
    /// The amount sent by the sender
    source_amount: u64,
    /// The [congestion controller](./../congestion/struct.CongestionController.html) for this stream
    congestion_controller: CongestionController,
    /// STREAM packets we have sent and have not received responses yet for
    pending_requests: Cell<Vec<PendingRequest>>,
    /// The [StreamDelivery](./struct.StreamDelivery.html) receipt of this stream
    receipt: StreamDelivery,
    /// Boolean indicating if the source account should also be sent to the receiver
    should_send_source_account: bool,
    /// The sequence number of this stream
    sequence: u64,
    /// The amount of rejected packets by the stream
    rejected_packets: u64,
    /// The STREAM error for this stream
    error: Option<Error>,
    /// The last time a packet was fulfilled for this stream.
    last_fulfill_time: Instant,
}

struct PendingRequest {
    sequence: u64,
    amount: u64,
    future: Pin<Box<dyn Future<Output = IlpResult> + Send>>,
}

/// The state of the send money future
#[derive(PartialEq)]
enum SendMoneyFutureState {
    /// Initial state of the future
    SendMoney = 0,
    /// Once the stream has been finished, it transitions to this state and tries to send a
    /// ConnectionCloseFrame
    Closing,
    /// The connection is now closed and the send_money function can return
    Closed,
}

#[project]
impl<S, A> SendMoneyFuture<S, A>
where
    S: IncomingService<A> + Send + Sync + Clone + 'static,
    A: Account + Send + Sync + 'static,
{
    /// Fire off requests until the congestion controller tells us to stop or we've sent the total amount or maximum time since last fulfill has elapsed
    fn try_send_money(&mut self) -> Result<bool, Error> {
        let mut sent_packets = false;
        loop {
            let amount = min(
                *self.source_amount,
                self.congestion_controller.get_max_amount(),
            );
            if amount == 0 {
                break;
            }
            *self.source_amount -= amount;

            // Load up the STREAM packet
            let sequence = self.next_sequence();
            let mut frames = vec![Frame::StreamMoney(StreamMoneyFrame {
                stream_id: 1,
                shares: 1,
            })];
            if *self.should_send_source_account {
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
            if let Some(ref next) = self.next {
                let mut next = next.clone();
                let from = self.from_account.clone();
                let request = Box::pin(async move {
                    next.handle_request(IncomingRequest { from, prepare }).await
                });
                self.pending_requests.get_mut().push(PendingRequest {
                    sequence,
                    amount,
                    future: request,
                });
                sent_packets = true;
            } else {
                panic!("Polled after finish");
            }
        }

        Ok(sent_packets)
    }

    /// Sends a STREAM inside a Prepare packet with a ConnectionClose frame to the peer
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
        if let Some(ref next) = self.next {
            let mut next = next.clone();
            let from = self.from_account.clone();
            let request =
                Box::pin(
                    async move { next.handle_request(IncomingRequest { from, prepare }).await },
                );
            self.pending_requests.get_mut().push(PendingRequest {
                sequence,
                amount: 0,
                future: request,
            });
        } else {
            panic!("Polled after finish");
        }
        Ok(())
    }

    fn poll_pending_requests(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Error>> {
        let pending_requests = self.pending_requests.take();
        let pending_requests = pending_requests
            .into_iter()
            .filter_map(
                |mut pending_request| match pending_request.future.as_mut().poll(cx) {
                    Poll::Pending => Some(pending_request),
                    Poll::Ready(result) => {
                        match result {
                            Ok(fulfill) => {
                                self.handle_fulfill(
                                    pending_request.sequence,
                                    pending_request.amount,
                                    fulfill,
                                );
                            }
                            Err(reject) => {
                                self.handle_reject(
                                    pending_request.sequence,
                                    pending_request.amount,
                                    reject,
                                );
                            }
                        };
                        None
                    }
                },
            )
            .collect();
        self.pending_requests.set(pending_requests);

        if let Some(error) = self.error.take() {
            error!("Send money stopped because of error: {:?}", error);
            Poll::Ready(Err(error))
        } else if self.pending_requests.get_mut().is_empty() {
            Poll::Ready(Ok(()))
        } else {
            Poll::Pending
        }
    }

    /// Parses the provided Fulfill packet.
    /// 1. Logs the fulfill in the congestion controller
    /// 1. Updates the last fulfill time of the send money future
    /// 1. It tries to aprse a Stream Packet inside the fulfill packet's data field
    ///    If successful, it increments the delivered amount by the Stream packet's prepare amount
    fn handle_fulfill(&mut self, sequence: u64, amount: u64, fulfill: Fulfill) {
        // TODO should we check the fulfillment and expiry or can we assume the plugin does that?
        self.congestion_controller.fulfill(amount);
        *self.should_send_source_account = false;
        *self.last_fulfill_time = Instant::now();

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

    /// Parses the provided Reject packet.
    /// 1. Increases the source-amount which was deducted at the start of the [send_money](./fn.send_money.html) loop
    /// 1. Logs the reject in the congestion controller
    /// 1. Increments the rejected packets counter
    /// 1. If the receipt's `delivered_asset` fields are not populated, it tries to parse
    ///    a Stream Packet inside the reject packet's data field to check if
    ///    there is a [`ConnectionAssetDetailsFrame`](./../packet/struct.ConnectionAssetDetailsFrame.html) frame.
    ///    If one is found, then it updates the receipt's `delivered_asset_scale` and `delivered_asset_code`
    ///    to them.
    fn handle_reject(&mut self, sequence: u64, amount: u64, reject: Reject) {
        *self.source_amount += amount;
        self.congestion_controller.reject(amount, &reject);
        *self.rejected_packets += 1;
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
                *self.error = Some(Error::SendMoneyError(format!(
                    "Packet was rejected with error: {} {}",
                    reject.code(),
                    str::from_utf8(reject.message()).unwrap_or_default(),
                )));
            }
        }
    }

    /// Increments the stream's sequence number and returns the updated value
    fn next_sequence(&mut self) -> u64 {
        let seq = *self.sequence;
        *self.sequence += 1;
        seq
    }
}

impl<S, A> Future for SendMoneyFuture<S, A>
where
    S: IncomingService<A> + Send + Sync + Clone + 'static,
    A: Account + Send + Sync + 'static,
{
    type Output = Result<(StreamDelivery, S), Error>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        // TODO maybe don't have loops here and in try_send_money
        let mut this = self.project();

        loop {
            ready!(this.poll_pending_requests(cx)?);
            if this.last_fulfill_time.elapsed() >= MAX_TIME_SINCE_LAST_FULFILL {
                return Poll::Ready(Err(Error::TimeoutError(format!(
                    "Time since last fulfill exceeded the maximum time limit of {:?} secs",
                    this.last_fulfill_time.elapsed().as_secs()
                ))));
            }

            if *this.source_amount == 0 && this.pending_requests.get_mut().is_empty() {
                if *this.state == SendMoneyFutureState::SendMoney {
                    *this.state = SendMoneyFutureState::Closing;
                    this.try_send_connection_close()?;
                } else {
                    *this.state = SendMoneyFutureState::Closed;
                    debug!(
                        "Send money future finished. Delivered: {} ({} packets fulfilled, {} packets rejected)", this.receipt.delivered_amount, *this.sequence - 1, this.rejected_packets,
                    );
                    return Poll::Ready(Ok((this.receipt.clone(), this.next.take().unwrap())));
                }
            } else if !this.try_send_money()? {
                return Poll::Pending;
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

    #[tokio::test]
    async fn stops_at_final_errors() {
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
        .await;
        assert!(result.is_err());
        assert_eq!(requests.lock().len(), 1);
    }
}
