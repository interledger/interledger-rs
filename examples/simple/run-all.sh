#! /bin/bash

sh kill-services.sh
sh start-services.sh
sh create-accounts.sh
sleep 1
sh check-balances.sh
sh send-payment.sh
sh check-balances.sh
# Comment out the next command if you want to keep the services running
sh kill-services.sh