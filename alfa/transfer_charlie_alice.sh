cargo run --bin ilp-cli -- \
  --node http://127.0.0.1:9770 pay charlie \
  --auth charlie_password \
  --amount 1000000 \
  --to http://127.0.0.1:7770/accounts/alice/spsp

# cargo run --bin ilp-cli -- --node http://127.0.0.1:8770 pay bob --auth bob_password --amount 20000000000000000 --to http://127.0.0.1:7770/accounts/alice/spsp
