#! /bin/bash

printf "Sending payment of 500 from Alice (on Node A) to Bob (on Node B)\n"
curl \
    -H "Authorization: Bearer alice" \
    -H "Content-Type: application/json" \
    -d '{"receiver":"http://localhost:8770/spsp/0","source_amount":500}' \
    http://localhost:7770/pay

printf "\n\n"
