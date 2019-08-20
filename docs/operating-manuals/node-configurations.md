
# Node Configurations

There are some parameters which determine how your node works. You could specify these parameters by the following methods.

1. By commandline arguments
1. By environment variables
1. By a configuration file

```bash #
# 1.
# passing as a commandline argument
# --{parameter name} {value}
cargo run --package interledger -- node --ilp_address example.alice --other_parameter other_value

# 2.
# passing as an environment variable
# {parameter name (typically in capital)}={value}
# note that the parameter names MUST begin with a prefix of "ILP_" e.g. ILP_SECRET_SEED
ILP_ADDRESS=example.alice OTHER_PARAMETER=other_value cargo run --package interledger -- node

# 3.
# passing by a YAML configuration file
cargo run --package interledger -- node --config config.yml
```

[YAML](https://yaml.org/) configuration files should be like:

```YAML
ilp_address: "example.alice"
other_parameter: "value"
```

### Parameters

The parameters are shown in the following format.

- `parameter name`
    - format
        - example
    - explanation

---

- `ilp_address`
    - [ILP Address](https://github.com/interledger/rfcs/blob/master/0015-ilp-addresses/0015-ilp-addresses.md)
        - `example.alice`
        - `g.your.node`
    - The ILP address of your node.
- `secret_seed`
    - 32 bytes HEX
        - `7823fb2888e60ec0352f2f7e49c4437128d59f137b37f172d2f67e744527c696`
    - The root secret used to derive encryption keys. This MUST NOT be changed after once you started up the node. You could obtain randomly generated one using `openssl rand -hex 32`.
- `admin_auth_token`
    - [`b64token`](https://tools.ietf.org/html/rfc6750#section-2.1)
        - `67ecb1754d514a3b23d0f72286e7b90c5543a532d33615cd800eaa831d50c9fe`
    - HTTP Authorization token for the node admin API. This should be sent as a Bearer token from clients. Refer to [Hypertext Transfer Protocol (HTTP/1.1): Authentication](https://tools.ietf.org/html/rfc7235) and [The OAuth 2.0 Authorization Framework: Bearer Token Usage](https://tools.ietf.org/html/rfc6750).
- `redis_connection`
    - Redis URI
        - `redis://127.0.0.1:6379`
        - `unix:/tmp/redis.sock`
    - Since Interledger.rs nodes currently store its data in Redis, we need a redis connection URI.
- `http_address`
    - IP address and port
        - `127.0.0.1:7770`
    - IP address and port to listen for HTTP connections. This is used for both the admin API and [ILP over HTTP](https://github.com/interledger/rfcs/blob/master/0035-ilp-over-http/0035-ilp-over-http.md) packets. ILP over HTTP is a means to transfer ILP packets instead of BTP connections.
- `settlement_address`
    - IP address and port
        - `127.0.0.1:7771`
    - IP address and port to listen for some of the [Settlement Engine API](https://github.com/interledger/rfcs/pull/536) calls. Requests will be posted by settlement engines.
- `btp_address`
    - IP address and port
        - `127.0.0.1:7768`
    - IP address and port to listen for BTP connections. Refer to [this](https://github.com/interledger/rfcs/blob/master/0033-relationship-between-protocols/0033-relationship-between-protocols.md#connections-1) brief explanation or [the RFC](https://github.com/interledger/rfcs/blob/master/0023-bilateral-transfer-protocol/0023-bilateral-transfer-protocol.md) to understand what BTP connections are.
- `default_spsp_account`
    - [URL-safe](https://tools.ietf.org/html/rfc3986#section-2.3) String
        - `f8b26fef-9a5f-4b1c-8004-538664140205`
    - When [SPSP payments](https://github.com/interledger/rfcs/blob/master/0009-simple-payment-setup-protocol/0009-simple-payment-setup-protocol.md) are sent to the root domain, the payment pointer is resolved to `<domain>/.well-known/pay`. This value determines which account those payments will be sent to.
- `route_broadcast_interval`
    - non-negative integer
        - `30000`
    - Interval, defined in milliseconds, on which the node will broadcast routing information to other nodes using CCP. Defaults to 30000ms (30 seconds).
