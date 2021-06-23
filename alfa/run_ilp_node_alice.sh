cargo run --bin ilp-node -- \
  --ilp_address example.alice \
  --secret_seed 8852500887504328225458511465394229327394647958135038836332350604 \
  --admin_auth_token hi_alice \
  --redis_url redis://127.0.0.1:6379/2 \
  --http_bind_address 127.0.0.1:7770 \
  --settlement_api_bind_address 127.0.0.1:7771 \
  --exchange_rate.provider CoinCap 
