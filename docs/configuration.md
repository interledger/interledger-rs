## How to Specify Configurations

The Interledger.rs node binary (`ilp-node`) can be configured in various ways:

### Environment variables

```bash #
# Passing as environment variables
# {parameter name (typically in capital)}={value}
# note that the parameter names MUST begin with a prefix of "ILP_" e.g. ILP_SECRET_SEED
ILP_ADDRESS=example.alice \
ILP_OTHER_PARAMETER=other_value \
ILP_PARENT__CHILD=hierarchical_value \
ilp-node
```

When you want to specify hierarchical parameters such as `bind_address` of `prometheus`, you have to set the parameter name as `ILP_PROMETHEUS__BIND_ADDRESS`, separating the parent and the child with `__` (two underscores). 

### Standard In (stdin)

```bash #
# Passing from STDIN in JSON, TOML, YAML format.
some_command | ilp-node
```

### Configuration files

```bash #
# Passing by a configuration file in JSON, TOML, YAML format.
# The first argument is the path to the configuration file.
ilp-node config.yml
```

### Command line arguments

```bash #
# Passing by command line arguments.
# --{parameter name} {value}
ilp-node --admin_auth_token super-secret --parent.child hierarchical_value
```

When you want to specify hierarchical parameters such as `bind_address` of `prometheus`, you have to set the parameter name as `prometheus.bind_address`, separating the parent and the child with `.` (a dot). 

Note that configurations are applied in the following order of priority:
1. Environment Variables
1. Stdin
1. Configuration files
1. Command line arguments.

## Configuration Parameters

The configuration parameters are explained in the following format.

- name of the config
    - format
    - example
    - explanation

---

### Required

- secret_seed
    - 32 bytes HEX
    - `fe6b34ed652486f38c95e9d761f737cf6473c52b2c8fd3a407fa775ea78e8c82`
    - A secret seed that is used to generate STREAM secrets and used to encrypt sensitive data. This MUST NOT be changed after once you started up the node. You could use `openssl rand -hex 32` to generate one.
- admin_auth_token
    - String
    - `naXg9PrfFAaY99s7`
    - An arbitrary secret token that is used for authenticating against administrative operations over the node's HTTP API. It must be passed as a Bearer token.

### Optional

- ilp_address
    - [ILP Addresses v2.0.0](https://github.com/interledger/rfcs/blob/master/0015-ilp-addresses/0015-ilp-addresses.md)
    - `g.my-node`
    - The ILP address of your node. The format should conform to the RFC above. If you are running a child node, you don't need to specify this.
- database_url
    - URL
    - `redis://127.0.0.1:6379`, `redis+unix:/tmp/redis.sock`
    - A URL of redis that the node connects to in order to store its data.
- http_bind_address
    - Socket Address (`address:port`)
    - `127.0.0.1:7770`
    - A pair of an IP address and a port to listen for HTTP connections. This is used for the HTTP API, ILP over HTTP packets and BTP connections. ILP over HTTP is a means to transfer ILP packets instead of BTP connections.
- settlement_api_bind_address
    - Socket Address (`address:port`)
    - `127.0.0.1:7771`
    - A pair of an IP address and a port to listen for connections from settlement engines. The address provides the Settlement Engine API.
- default_spsp_account
    - String (should be an existing account username)
    - `my_account`
    - When SPSP payments are sent to the root domain, the payment pointer is resolved to `<domain>/.well-known/pay` (if not provided, this endpoint will not be exposed). This value determines which account those payments will be sent to.
- route_broadcast_interval
    - Non-negative Integer (in milliseconds)
    - `30000`
    - Interval, defined in milliseconds, on which the node will broadcast routing information to other nodes using CCP. Defaults to 30000ms (30 seconds).
- exchange_rate
    - provider
        - String (should be one of `CoinCap`, `CryptoCompare`)
        - `CoinCap`
        - Exchange rate API to poll for exchange rates. If this is not set, the node will not poll for rates and will instead use the rates set via the HTTP API. Note that [CryptoCompare](#using-cryptocompare) can also be used **when the node is configured via a config file or stdin**, because an API key must be provided to use that service.
    - poll_interval
        - Non-negative Integer (in milliseconds)
        - `60000`
        - Interval, defined in milliseconds, on which the node will poll the `provider` (if specified) for exchange rates.
    - spread
        - Float
        - `0.01`
        - Spread, as a fraction, to add on top of the exchange rate. This amount is kept as the node operator's profit, or may cover fluctuations in exchange rates. For example, take an incoming packet with an amount of 100. If the exchange rate is 1:0.5 and the spread is 0.01, the amount on the outgoing packet would be 198 (instead of 200 without the spread).
- [prometheus](https://prometheus.io/)
    - bind_address
        - Socket Address (`address:port`)
        - `9654`
        - IP address and port to host the Prometheus exporter on.
    - histogram_window
        - Non-negative Integer (in milliseconds)
        - `300000`
        - Amount of time, in milliseconds, that the node will collect data points for the Prometheus histograms. Defaults to 300000ms (5 minutes).
    - histogram_granularity
        - Non-negative Integer (in milliseconds)
        - `10000`
        - Granularity, in milliseconds, that the node will use to roll off old data. For example, a value of 1000ms (1 second) would mean that the node forgets the oldest 1 second of histogram data points every second. Defaults to 10000ms (10 seconds).

#### Using CryptoCompare 

You have to use a config file or STDIN to use `CryptoCompare` as a rate provider as follows.

```bash #
# run ilp-node with STDIN
some-command | ilp-node
```

Then `some-command` should output a config like:

```yaml
exchange_rate.provider:
  CryptoCompare: insert_api_key_here
```

It is recommended to pass the API key from STDIN because passing from arguments might expose the secret unexpectedly, for example using `history`.
