cargo run --bin ilp-cli -- \
  --node http://127.0.0.1:7770 accounts create alice \
  --auth hi_alice \
  --ilp-address example.alice \
  --asset-code ETH \
  --asset-scale 18 \
  --ilp-over-http-incoming-token alice_password

cargo run --bin ilp-cli -- \
  --node http://127.0.0.1:8770 accounts create bob \
  --auth hi_bob \
  --ilp-address example.bob \
  --asset-code ETH \
  --asset-scale 18 \
  --ilp-over-http-incoming-token bob_password

cargo run  --bin ilp-cli -- \
  --node http://127.0.0.1:7770 accounts create bob \
  --auth hi_alice \
  --ilp-address example.bob \
  --asset-code ETH \
  --asset-scale 18 \
  --settlement-engine-url http://127.0.0.1:3000 \
  --ilp-over-http-incoming-token bob_password \
  --ilp-over-http-outgoing-token alice_password \
  --ilp-over-http-url http://127.0.0.1:8770/accounts/alice/ilp \
  --settle-threshold 5000000000000000 \
  --settle-to 0 \
  --routing-relation Peer

cargo run  --bin ilp-cli -- \
  --node http://127.0.0.1:8770 accounts create alice \
  --auth hi_bob \
  --ilp-address example.alice \
  --asset-code ETH \
  --asset-scale 18 \
  --settlement-engine-url http://127.0.0.1:3001 \
  --ilp-over-http-incoming-token alice_password \
  --ilp-over-http-outgoing-token bob_password \
  --ilp-over-http-url http://127.0.0.1:7770/accounts/bob/ilp \
  --settle-threshold 5000000000000000 \
  --settle-to 0 \
  --routing-relation Peer

# printf "Adding Alice's account...\n"
# ilp-cli accounts create alice \
#     --ilp-address example.alice \
#     --asset-code ETH \
#     --asset-scale 18 \
#     --max-packet-amount 100 \
#     --ilp-over-http-incoming-token in_alice \
#     --settle-to 0 &> logs/account-alice-alice.log

# printf "Adding Bob's Account...\n"
# ilp-cli --node http://localhost:8770 accounts create bob \
#     --auth hi_bob \
#     --ilp-address example.bob \
#     --asset-code ETH \
#     --asset-scale 18 \
#     --max-packet-amount 100 \
#     --ilp-over-http-incoming-token in_bob \
#     --settle-to 0 &> logs/account-bob-bob.log

# printf "Adding Bob's account on Alice's node...\n"
# ilp-cli accounts create bob \
#     --ilp-address example.bob \
#     --asset-code ETH \
#     --asset-scale 18 \
#     --max-packet-amount 100 \
#     --settlement-engine-url http://localhost:3000 \
#     --ilp-over-http-incoming-token bob_password \
#     --ilp-over-http-outgoing-token alice_password \
#     --ilp-over-http-url http://localhost:8770/accounts/alice/ilp \
#     --settle-threshold 500 \
#     --min-balance -1000 \
#     --settle-to 0 \
#     --routing-relation Peer &> logs/account-alice-bob.log &

# printf "Adding Alice's account on Bob's node...\n"
# ilp-cli --node http://localhost:8770 accounts create alice \
#     --auth hi_bob \
#     --ilp-address example.alice \
#     --asset-code ETH \
#     --asset-scale 18 \
#     --max-packet-amount 100 \
#     --settlement-engine-url http://localhost:3001 \
#     --ilp-over-http-incoming-token alice_password \
#     --ilp-over-http-outgoing-token bob_password \
#     --ilp-over-http-url http://localhost:7770/accounts/bob/ilp \
#     --settle-threshold 500 \
#     --settle-to 0 \
#     --routing-relation Peer &> logs/account-bob-alice.log &