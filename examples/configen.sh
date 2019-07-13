#!/bin/bash

random-string() {
    cat /dev/urandom | LC_CTYPE=C tr -dc '0-9' | fold -w 256 | head -n 1 | head -c $1
}

admin_token=$(random-string 64)
secret_seed=$(random-string 64)
node_id=$(random-string 6)
ilp_address="test.$node_id"
redis_connection="redis://127.0.0.1:${1:-6379}" # use first cli arg or default
btp_api="127.0.0.1:${2:-7768}"
http_api="127.0.0.1:${3:-7770}"
settlement_api="127.0.0.1:${4:-7771}"

# cat > ./examples/config-$(date +%s).yml << EOL
output="address: \"$ilp_address\"
secret_seed: \"$secret_seed\"
admin_auth_token: \"$admin_token\"
redis_connection: \"$redis_connection\"
btp_address: \"$btp_api\"
settlement_address: \"$settlement_api\"
http_address: \"$http_api\""

echo "$output"
