# Interledger CLI

Command Line Interface which makes calls over HTTP to the Interledger node.

Build yourself:
```bash
cargo build --bin ilp-cli
```

Run via docker:

```bash
docker pull interledgerrs/ilp-cli
docker run -it interledgerrs/ilp-cli
```


Example output:

```bash
$ cargo run --bin ilp-cli --help

ilp-cli 0.0.1
Interledger.rs Command-Line Interface

USAGE:
    ilp-cli [FLAGS] [OPTIONS] [SUBCOMMAND]

FLAGS:
    -h, --help       Prints help information
    -q, --quiet      Disable printing the bodies of successful HTTP responses upon receipt
    -V, --version    Prints version information

OPTIONS:
        --node <node_url>    The base URL of the node to connect to [env: ILP_CLI_NODE_URL=]  [default:
                             http://localhost:7770]

SUBCOMMANDS:
    accounts              Operations for interacting with accounts
    help                  Prints this message or the help of the given subcommand(s)
    pay                   Send a payment from an account on this node
    rates                 Operations for interacting with exchange rates
    routes                Operations for interacting with the routing table
    settlement-engines    Interact with the settlement engine configurations
    status                Query the status of the server
    testnet               Easily access the testnet
```