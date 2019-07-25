#! /bin/bash

printf "Checking balances...\n"

printf "\nAlice's balance: "
curl \
-H "Authorization: Bearer admin-a" \
http://localhost:7770/accounts/0/balance

printf "\nNode B's balance on Node A: "
curl \
-H "Authorization: Bearer admin-a" \
http://localhost:7770/accounts/1/balance

printf "\nNode A's balance on Node B: "
curl \
-H "Authorization: Bearer admin-b" \
http://localhost:8770/accounts/1/balance

printf "\nBob's balance: "
curl \
-H "Authorization: Bearer admin-b" \
http://localhost:8770/accounts/0/balance

printf "\n\n"
