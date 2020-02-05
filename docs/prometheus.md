# Prometheus Monitoring

When the binary is compiled with the `--features "monitoring"` flag, then [Prometheus](https://prometheus.io/) will be enabled.

From the website:

> Prometheus is a systems and service monitoring system. It collects metrics from configured targets at given intervals, evaluates rule expressions, displays the results, and can trigger alerts if some condition is observed to be true.

We combine Prometheus and [Metrics](https://github.com/metrics-rs/metrics) in order to get detailed metrics about our node's performance and expose them via an API endpoint, which may then be consumed by other services.

To enable Prometheus, provide an extra section with Prometheus-related configuration options in your config file (or CLI arguments).

```yaml
PrometheusConfig:
    bind_address: 127.0.0.1:9999
    histogram_window: 5000
    histogram_granularity: 1000
```

This will open an endpoint at `http://127.0.0.1:9999/` which you can query to get the current data gathered by our instrumentation system exposed via Prometheus.

For each request, we do the following:
1. Increment the number of prepare packets for the type of request
1. Monitor the time (in nanonseconds) required to handle the request
1. Increment the number of fulfill (or reject, depending on the result of the previous step) packets for the type of request

Each of the above logs is labelled with the sending account's asset code and routing relation if it comes from an Incoming request. If it is an outgoing request, then we also label it with the receiving account's asset code and routing relation.

Example output below:

```
$ curl localhost:9999/

# metrics snapshot (ts=1580809069) (prometheus exposition format)
# TYPE requests_outgoing_fulfill counter
requests_outgoing_fulfill{from_asset_code="ABC",to_asset_code="ABC",from_routing_relation="NonRoutingAccount",to_routing_relation="NonRoutingAccount"} 1

# TYPE requests_incoming_prepare counter
requests_incoming_prepare{from_asset_code="ABC",from_routing_relation="NonRoutingAccount"} 3

# TYPE requests_incoming_reject counter
requests_incoming_reject{from_asset_code="ABC",from_routing_relation="NonRoutingAccount"} 1

# TYPE requests_outgoing_prepare counter
requests_outgoing_prepare{from_asset_code="ABC",to_asset_code="ABC",from_routing_relation="NonRoutingAccount",to_routing_relation="NonRoutingAccount"} 2

# TYPE requests_incoming_fulfill counter
requests_incoming_fulfill{from_asset_code="ABC",from_routing_relation="NonRoutingAccount"} 2

# TYPE requests_outgoing_reject counter
requests_outgoing_reject{from_asset_code="ABC",to_asset_code="ABC",from_routing_relation="NonRoutingAccount",to_routing_relation="NonRoutingAccount"} 1

# TYPE requests_incoming_duration summary
requests_incoming_duration{from_asset_code="ABC",from_routing_relation="NonRoutingAccount",quantile="0"} 365824
requests_incoming_duration{from_asset_code="ABC",from_routing_relation="NonRoutingAccount",quantile="0.5"} 20922367
requests_incoming_duration{from_asset_code="ABC",from_routing_relation="NonRoutingAccount",quantile="0.9"} 22249471
requests_incoming_duration{from_asset_code="ABC",from_routing_relation="NonRoutingAccount",quantile="0.95"} 22249471
requests_incoming_duration{from_asset_code="ABC",from_routing_relation="NonRoutingAccount",quantile="0.99"} 22249471
requests_incoming_duration{from_asset_code="ABC",from_routing_relation="NonRoutingAccount",quantile="0.999"} 22249471
requests_incoming_duration{from_asset_code="ABC",from_routing_relation="NonRoutingAccount",quantile="1"} 22249471
requests_incoming_duration_sum{from_asset_code="ABC",from_routing_relation="NonRoutingAccount"} 43528460
requests_incoming_duration_count{from_asset_code="ABC",from_routing_relation="NonRoutingAccount"} 3

# TYPE requests_outgoing_duration summary
requests_outgoing_duration{from_asset_code="ABC",to_asset_code="ABC",from_routing_relation="NonRoutingAccount",to_routing_relation="NonRoutingAccount",quantile="0"} 14123008
requests_outgoing_duration{from_asset_code="ABC",to_asset_code="ABC",from_routing_relation="NonRoutingAccount",to_routing_relation="NonRoutingAccount",quantile="0.5"} 14131199
requests_outgoing_duration{from_asset_code="ABC",to_asset_code="ABC",from_routing_relation="NonRoutingAccount",to_routing_relation="NonRoutingAccount",quantile="0.9"} 16744447
requests_outgoing_duration{from_asset_code="ABC",to_asset_code="ABC",from_routing_relation="NonRoutingAccount",to_routing_relation="NonRoutingAccount",quantile="0.95"} 16744447
requests_outgoing_duration{from_asset_code="ABC",to_asset_code="ABC",from_routing_relation="NonRoutingAccount",to_routing_relation="NonRoutingAccount",quantile="0.99"} 16744447
requests_outgoing_duration{from_asset_code="ABC",to_asset_code="ABC",from_routing_relation="NonRoutingAccount",to_routing_relation="NonRoutingAccount",quantile="0.999"} 16744447
requests_outgoing_duration{from_asset_code="ABC",to_asset_code="ABC",from_routing_relation="NonRoutingAccount",to_routing_relation="NonRoutingAccount",quantile="1"} 16744447
requests_outgoing_duration_sum{from_asset_code="ABC",to_asset_code="ABC",from_routing_relation="NonRoutingAccount",to_routing_relation="NonRoutingAccount"} 30871847
requests_outgoing_duration_count{from_asset_code="ABC",to_asset_code="ABC",from_routing_relation="NonRoutingAccount",to_routing_relation="NonRoutingAccount"} 2
```
