use crate::InterledgerNode;
use metrics_core::{Builder, Drain, Observe};
use metrics_runtime;
use serde::Deserialize;
use std::{net::SocketAddr, sync::Arc, time::Duration};
use tracing::{error, info};
use warp::{
    http::{Response, StatusCode},
    Filter,
};

/// Configuration for [Prometheus](https://prometheus.io) metrics collection.
#[derive(Deserialize, Clone)]
pub struct PrometheusConfig {
    /// IP address and port to host the Prometheus endpoint on.
    pub bind_address: SocketAddr,
    /// Amount of time, in milliseconds, that the node will collect data points for the
    /// Prometheus histograms. Defaults to 300000ms (5 minutes).
    #[serde(default = "PrometheusConfig::default_histogram_window")]
    pub histogram_window: u64,
    /// Granularity, in milliseconds, that the node will use to roll off old data.
    /// For example, a value of 1000ms (1 second) would mean that the node forgets the oldest
    /// 1 second of histogram data points every second. Defaults to 10000ms (10 seconds).
    #[serde(default = "PrometheusConfig::default_histogram_granularity")]
    pub histogram_granularity: u64,
}

impl PrometheusConfig {
    fn default_histogram_window() -> u64 {
        300_000
    }

    fn default_histogram_granularity() -> u64 {
        10_000
    }
}

/// Starts a Prometheus metrics server that will listen on the configured address.
///
/// # Errors
/// This will fail if another Prometheus server is already running in this
/// process or on the configured port.
#[allow(clippy::cognitive_complexity)]
pub async fn serve_prometheus(node: InterledgerNode) -> Result<(), ()> {
    let prometheus = if let Some(ref prometheus) = node.prometheus {
        prometheus
    } else {
        error!(target: "interledger-node", "No prometheus configuration provided");
        return Err(());
    };

    // Set up the metrics collector
    let receiver = metrics_runtime::Builder::default()
        .histogram(
            Duration::from_millis(prometheus.histogram_window),
            Duration::from_millis(prometheus.histogram_granularity),
        )
        .build()
        .expect("Failed to create metrics Receiver");

    let controller = receiver.controller();
    // Try installing the global recorder
    match metrics::set_boxed_recorder(Box::new(receiver)) {
        Ok(_) => {
            let observer = Arc::new(metrics_runtime::observers::PrometheusBuilder::default());

            let filter = warp::get().and(warp::path::end()).map(move || {
                let mut observer = observer.build();
                controller.observe(&mut observer);
                let prometheus_response = observer.drain();
                Response::builder()
                    .status(StatusCode::OK)
                    .header("Content-Type", "text/plain; version=0.0.4")
                    .body(prometheus_response)
            });

            info!(target: "interledger-node",
                "Prometheus metrics server listening on: {}",
                prometheus.bind_address
            );

            tokio::spawn(warp::serve(filter).bind(prometheus.bind_address));
            Ok(())
        }
        Err(e) => {
            error!(target: "interledger-node", "Error installing global metrics recorder (this is likely caused by trying to run two nodes with Prometheus metrics in the same process): {:?}", e);
            Err(())
        }
    }
}
