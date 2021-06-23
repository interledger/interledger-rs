cargo run --bin ilp-node -- \
  --ilp_address example.charlie \
  --secret_seed 1232362131122139900555208458637022875563691455429373719368053354 \
  --admin_auth_token hi_charlie \
  --redis_url redis://127.0.0.1:6379/4 \
  --http_bind_address 127.0.0.1:9770 \
  --settlement_api_bind_address 127.0.0.1:9771 \
  --exchange_rate.provider CoinCap 
