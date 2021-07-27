use super::{fixtures::*, redis_helpers::*};
use futures::{FutureExt, StreamExt};
use interledger_api::NodeStore;
use interledger_packet::Address;
use interledger_service::Account as AccountTrait;
use interledger_store::redis::RedisStoreBuilder;
use interledger_stream::{PaymentNotification, StreamNotificationsStore};
use std::str::FromStr;
use tokio_stream::wrappers::BroadcastStream;

#[tokio::test]
async fn notifications_on_multitenant_config() {
    let context = TestContext::new();

    let first = RedisStoreBuilder::new(context.get_client_connection_info(), [0; 32])
        .with_db_prefix("first")
        .connect()
        .await
        .unwrap();

    let second = RedisStoreBuilder::new(context.get_client_connection_info(), [1; 32])
        .with_db_prefix("second")
        .connect()
        .await
        .unwrap();

    let firstuser = first
        .insert_account(ACCOUNT_DETAILS_0.clone())
        .await
        .unwrap();

    let seconduser = second
        .insert_account({
            let mut details = ACCOUNT_DETAILS_1.clone();
            details.ilp_address = Some(Address::from_str("example.charlie").unwrap());
            details
        })
        .await
        .unwrap();

    let first_pmt = PaymentNotification {
        from_username: seconduser.username().to_owned(),
        to_username: firstuser.username().to_owned(),
        destination: firstuser.ilp_address().to_owned(),
        amount: 1,
        timestamp: String::from("2021-04-04T12:11:11.987+00:00"),
        sequence: 2,
        connection_closed: false,
    };

    let second_pmt = PaymentNotification {
        from_username: firstuser.username().to_owned(),
        to_username: seconduser.username().to_owned(),
        destination: seconduser.ilp_address().to_owned(),
        amount: 1,
        timestamp: String::from("2021-04-04T12:11:10.987+00:00"),
        sequence: 1,
        connection_closed: false,
    };

    // do the test in a loop since sometimes the psubscribe functionality just isn't ready
    for _ in 0..10 {
        // we recreate these on the start of every attempt in order to get a fresh start; the
        // channel will not forward us messages which have come before.
        let mut rx1 = BroadcastStream::new(first.all_payment_subscription());
        let mut rx2 = BroadcastStream::new(second.all_payment_subscription());

        first.publish_payment_notification(first_pmt.clone());
        second.publish_payment_notification(second_pmt.clone());

        // these used to only log before #700:
        //
        // WARN interledger_store::redis: Ignoring unexpected message from Redis subscription for channel: first:stream_notifications:...
        // WARN interledger_store::redis: Ignoring unexpected message from Redis subscription for channel: second:stream_notifications:...
        //
        // after fixing this, there will still be:
        //
        // TRACE interledger_store::redis: Ignoring message for account ... because there were no open subscriptions
        // TRACE interledger_store::redis: Ignoring message for account ... because there were no open subscriptions
        //
        // even though the subscription to all exists. this tests uses the all_payment_subscription()
        // and that should be ok, since the trigger still comes through PSUBSCRIBE.
        //
        let deadline = std::time::Duration::from_millis(1000);
        let read_both = futures::future::join(rx1.next(), rx2.next());

        let (msg1, msg2) = match tokio::time::timeout(deadline, read_both).await {
            Ok(messages) => messages,
            Err(tokio::time::error::Elapsed { .. }) => {
                // failure is most likely because of redis, or publishing to it
                // see issue #711.
                continue;
            }
        };
        assert_eq!(msg1.unwrap().expect("cannot lag yet").sequence, 2);
        assert_eq!(msg2.unwrap().expect("cannot lag yet").sequence, 1);

        let (msg1, msg2) = (rx1.next().now_or_never(), rx2.next().now_or_never());
        assert!(msg1.is_none(), "{:?}", msg1);
        assert!(msg2.is_none(), "{:?}", msg2);
        return;
    }

    unreachable!("did not complete with retries");
}
