#!/usr/bin/env node

import * as yargs from 'yargs'
import { XrpSettlementEngine } from './index'

const testnetServer = 'wss://s.altnet.rippletest.net:51233'

const argv = yargs.option('address', {
    description: 'XRP address to settle from and listen to for incoming payments',
    type: 'string'
}).option('secret', {
    description: 'XRP account secret',
    type: 'string'
}).option('testnet', {
    default: false,
    type: 'boolean',
    description: 'Connect to the XRP testnet instead of the main net'
}).option('rippled', {
    description: 'Websocket URI of the rippled server to connect to',
    default: 'wss://s1.ripple.com',
    type: 'string'
}).option('redis', {
    description: 'Redis URI to connect to',
    // TODO maybe connect using a socket instead
    default: 'redis://localhost:6379',
    type: 'string'
}).option('min_settlement_amount', {
    default: 1000000,
    type: 'number',
    description: 'Minimum amount, denominated in XRP drops (0.000001 XRP), to send in an XRP payment'
}).option('poll_interval', {
    default: 60000,
    type: 'number',
    description: 'Interval, denominated in milliseconds, to poll the Redis database for changes in account balances'
}).require(['address', 'secret'])
    .argv

const config = {
  address: argv.address,
  secret: argv.secret,
  rippledUri: (argv.testnet ? testnetServer : argv.rippled),
  redisUri: argv.redis,
  minSettlementAmount: argv.min_settlement_amount,
  pollInterval: argv.poll_interval
}

const engine = new XrpSettlementEngine(config)
engine.connect().then(() => {
  console.log('Listening for incoming XRP payments and polling Redis for accounts that need to be settled')
}).catch((err) => console.error(err))
