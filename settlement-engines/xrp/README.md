# XRP Settlement Engine
> Settle ILP packets sent/received by Interledger.js

This Settlement Engine uses on-ledger XRP payments to settle balances and connects to the same Redis instance Interledger.rs uses to track account balances.

## Build
```bash
$> npm install
$> npm run build
$> npm link
```

## Generate XRPL Test Credentials
In order to run the Settlement Engine, generate XRP Ledger test credentials using the [XRP Test Net Faucet](https://developers.ripple.com/xrp-test-net-faucet.html).

## Run 
Run the Settlement Engine using the supplied XRPL **Test Net** credentials:
```bash
$> npm start --  --address rpexMDP9d2aqgiv1bxSJo1MGwdeEFQfszX --secret ssj6KU1JeSS8NBznmX8JYryX9WuDn --testnet`
```

## View CLI Options
To view customizable options available when running the Settlement Engine, issue the following command:

```bash
$>  npm start
```
