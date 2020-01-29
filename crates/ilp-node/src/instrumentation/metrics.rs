use interledger::{
    ccp::CcpRoutingAccount,
    service::{
        Account, IlpResult, IncomingRequest, IncomingService, OutgoingRequest, OutgoingService,
    },
};
use metrics::{self, labels, recorder, Key};
use std::time::Instant;

pub async fn incoming_metrics<A: Account + CcpRoutingAccount>(
    request: IncomingRequest<A>,
    mut next: Box<dyn IncomingService<A> + Send>,
) -> IlpResult {
    let labels = labels!(
        "from_asset_code" => request.from.asset_code().to_string(),
        "from_routing_relation" => request.from.routing_relation().to_string(),
    );
    recorder().increment_counter(
        Key::from_name_and_labels("requests.incoming.prepare", labels.clone()),
        1,
    );
    let start_time = Instant::now();

    let result = next.handle_request(request).await;
    if result.is_ok() {
        recorder().increment_counter(
            Key::from_name_and_labels("requests.incoming.fulfill", labels.clone()),
            1,
        );
    } else {
        recorder().increment_counter(
            Key::from_name_and_labels("requests.incoming.reject", labels.clone()),
            1,
        );
    }

    recorder().record_histogram(
        Key::from_name_and_labels("requests.incoming.duration", labels),
        (Instant::now() - start_time).as_nanos() as u64,
    );
    result
}

pub async fn outgoing_metrics<A: Account + CcpRoutingAccount>(
    request: OutgoingRequest<A>,
    mut next: Box<dyn OutgoingService<A> + Send>,
) -> IlpResult {
    let labels = labels!(
        "from_asset_code" => request.from.asset_code().to_string(),
        "to_asset_code" => request.to.asset_code().to_string(),
        "from_routing_relation" => request.from.routing_relation().to_string(),
        "to_routing_relation" => request.to.routing_relation().to_string(),
    );

    // TODO replace these calls with the counter! macro if there's a way to easily pass in the already-created labels
    // right now if you pass the labels into one of the other macros, it gets a recursion limit error while expanding the macro
    recorder().increment_counter(
        Key::from_name_and_labels("requests.outgoing.prepare", labels.clone()),
        1,
    );
    let start_time = Instant::now();

    let result = next.send_request(request).await;
    if result.is_ok() {
        recorder().increment_counter(
            Key::from_name_and_labels("requests.outgoing.fulfill", labels.clone()),
            1,
        );
    } else {
        recorder().increment_counter(
            Key::from_name_and_labels("requests.outgoing.reject", labels.clone()),
            1,
        );
    }

    recorder().record_histogram(
        Key::from_name_and_labels("requests.outgoing.duration", labels.clone()),
        (Instant::now() - start_time).as_nanos() as u64,
    );

    result
}
