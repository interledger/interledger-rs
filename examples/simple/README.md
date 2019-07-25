# Simple Two-Node Payment

This example sets up two Interledger.rs nodes, connects them together, and sends a payment from one to the other.

To run the full example, use the [`run-all.sh`](./run-all.sh) script. Otherwise, you can walk through each step below.

Each of the services write their logs to files found under the `logs` directory. You can run `tail -f logs/node-a.log`, for example, to watch the logs of Node A.

## Running and Configuring the Nodes

The [`start-services.sh`](./start-services.sh) script runs a Redis instance and the two nodes, Node A and Node B.

The [`create-accounts.sh`](./create-accounts.sh) script sets up accounts for two users, Alice and Bob. It also creates accounts that represent the connection between Nodes A and B.

## Sending a Payment

The [`send-payment.sh`](./send-payment.sh) script sends a payment from Alice to Bob that is routed from Node A to Node B.

You can run the [`check-balances.sh`](./check-balances.sh) script to print each of the accounts' balances (try doing this before and after sending a payment).

That's it for this example! Check out the [other examples](../README.md) for more complex demos that show more of the features of Interledger, including multi-hop routing and cross-currency payments.
