import { RippleAPI } from 'ripple-lib'
import { RedisClient, createClient } from 'redis'
import { readFile } from 'fs'
import { promisify } from 'util'
import Debug from 'debug'
import * as path from 'path'
import { createHash } from 'crypto'
const debug = Debug('xrp-settlement-engine')
const readFileAsync = promisify(readFile)

// TODO should the settlement engine go through and make sure all of the xrp_addresses are stored in this hash map?
const DEFAULT_POLL_INTERVAL = 60000
const KEY_ARGS = 0
const UNCLAIMED_BALANCE_KEY = 'unclaimed_balances:xrp'

export interface XrpSettlementEngineConfig {
  address: string,
  secret: string,
  rippledUri?: string,
  redisUri?: string,
  minSettlementDrops?: number,
  pollInterval?: number
}

export class XrpSettlementEngine {
  private rippleClient: RippleAPI
  private redisClient: RedisClient
  private address: string
  private secret: string
  private minDropsToSettle: number
  private pollInterval: number
  private interval: NodeJS.Timeout
  // TODO add type annotations to these arrow functions
  private getAccountsThatNeedSettlement: any
  private creditAccountForSettlement: any
  private updateBalanceAfterSettlement: any
  private addUnclaimedBalance: any

  constructor (config: XrpSettlementEngineConfig) {
    this.address = config.address
    this.secret = config.secret
    this.minDropsToSettle = (typeof config.minSettlementDrops === 'number' ? config.minSettlementDrops : 1000000)
    this.redisClient = createClient(config.redisUri || 'redis://localhost:6379')
    this.rippleClient = new RippleAPI({
      server: config.rippledUri || 'wss://s.altnet.rippletest.net:51233'// 'wss://s1.ripple.com'
    })
    this.pollInterval = config.pollInterval || DEFAULT_POLL_INTERVAL
  }

  async connect (): Promise<void> {
    // Connect to redis and rippled
    await Promise.all([
      this.rippleClient.connect().then(() => debug('Connected to rippled')),
      new Promise((resolve, reject) => {
        this.redisClient.once('ready', resolve)
        this.redisClient.once('error', reject)
      }).then(() => {
        debug('Connected to Redis')
        return this.loadScripts()
      })
    ])

    // Set up the settlement engine to poll the accounts for balance changes
    debug(`Setting up to poll for balance changes every ${this.pollInterval}ms`)
    await this.checkAccounts()
    this.interval = setInterval(() => this.checkAccounts(), this.pollInterval)

    // Subscribe to rippled events to be notified of incoming payments
    this.rippleClient.connection.on('transaction', this.handleTransaction.bind(this))
    await this.rippleClient.request('subscribe', {
      accounts: [this.address]
    })
  }

  private async loadScripts () {
    debug('Loading scripts into Redis')

    const loadScript = promisify(this.redisClient.script.bind(this.redisClient, 'load'))

    const checkAccountsScript = await readFileAsync(path.join(__dirname, '../scripts/get_accounts_that_need_settlement.lua'), 'utf8')
    await loadScript(checkAccountsScript)
    const checkAccountsScriptHash = createHash('sha1').update(checkAccountsScript).digest('hex')
    this.getAccountsThatNeedSettlement = promisify(this.redisClient.evalsha.bind(this.redisClient, checkAccountsScriptHash, KEY_ARGS))

    const creditAccountForSettlementScript = await readFileAsync(path.join(__dirname, '../scripts/credit_account_for_settlement.lua'), 'utf8')
    await loadScript(creditAccountForSettlementScript)
    const creditAccountForSettlementScriptHash = createHash('sha1').update(creditAccountForSettlementScript).digest('hex')
    this.creditAccountForSettlement = promisify(this.redisClient.evalsha.bind(this.redisClient, creditAccountForSettlementScriptHash, KEY_ARGS))

    const updateBalanceAfterSettlementScript = await readFileAsync(path.join(__dirname, '../scripts/update_balance_after_settlement.lua'), 'utf8')
    await loadScript(updateBalanceAfterSettlementScript)
    const updateBalanceAfterSettlementScriptHash = createHash('sha1').update(updateBalanceAfterSettlementScript).digest('hex')
    this.updateBalanceAfterSettlement = promisify(this.redisClient.evalsha.bind(this.redisClient, updateBalanceAfterSettlementScriptHash, KEY_ARGS))

    this.addUnclaimedBalance = promisify(this.redisClient.hincrby.bind(this.redisClient, UNCLAIMED_BALANCE_KEY))

    debug('Loaded scripts')
  }

  async disconnect (): Promise<void> {
    debug('Disconnecting')
    if (this.interval) {
      clearInterval(this.interval)
    }
    await Promise.all([
      this.rippleClient.disconnect().then(() => debug('Disconnected from rippled')),
      promisify(this.redisClient.quit.bind(this.redisClient))().then(() => debug('Disconnected from Redis'))
    ])
    debug('Disconnected')
  }

  private async checkAccounts () {
    debug('Checking accounts')
    let cursor = '0'
    do {
      cursor = await this.scanAccounts(cursor)
    }
    while (cursor !== '0')
  }

  private async scanAccounts (cursor: string): Promise<string> {
    const [newCursor, accountsToSettle] = await this.getAccountsThatNeedSettlement(cursor, 20, this.minDropsToSettle)

    for (let accountRecord of accountsToSettle) {
      const [account, xrpAddress, dropsToSettle] = accountRecord
      this.settle(account, xrpAddress, dropsToSettle)
    }

    return newCursor
  }

  private async settle (account: string, xrpAddress: string, drops: string) {
    debug(`Attempting to send ${drops} XRP drops to account: ${account} (XRP address: ${xrpAddress})`)
    try {
      const payment = await this.rippleClient.preparePayment(this.address, {
        source: {
          address: this.address,
          amount: {
            value: '' + drops,
            currency: 'drops'
          }
        },
        destination: {
          address: xrpAddress,
          minAmount: {
            value: '' + drops,
            currency: 'drops'
          }
        }
      }, {
          // TODO add max fee
        maxLedgerVersionOffset: 5
      })
      const { signedTransaction } = this.rippleClient.sign(payment.txJSON, this.secret)
      const result = await this.rippleClient.submit(signedTransaction)
      if (result.resultCode === 'tesSUCCESS') {
        debug(`Sent ${drops} drop payment to account: ${account} (xrpAddress: ${xrpAddress})`)
        const newBalance = await this.updateBalanceAfterSettlement(account, drops)
        debug(`Account ${account} now has balance: ${newBalance}`)
      }
    } catch (err) {
      console.error(`Error preparing and submitting payment to rippled. Settlement to account: ${account} (xrpAddress: ${xrpAddress}) for ${drops} drops failed:`, err)
    }
  }

  private async handleTransaction (tx: any) {
    if (!tx.validated || tx.engine_result !== 'tesSUCCESS' || tx.transaction.TransactionType !== 'Payment' || tx.transaction.Destination !== this.address) {
      return
    }

    // Parse amount received from transaction
    let drops
    try {
      if (tx.meta.delivered_amount) {
        drops = parseInt(tx.meta.delivered_amount, 10)
      } else {
        drops = parseInt(tx.transaction.Amount, 10)
      }
    } catch (err) {
      console.error('Error parsing amount received from transaction: ', tx)
      return
    }

    const fromAddress = tx.transaction.Account
    debug(`Got incoming XRP payment for ${drops} drops from XRP address: ${fromAddress}`)

    try {
      const [account, newBalance] = await this.creditAccountForSettlement(fromAddress, drops)
      debug(`Credited account: ${account} for incoming settlement, balance is now: ${newBalance}`)
    } catch (err) {
      if (err.message.includes('No account associated')) {
        debug(`No account associated with address: ${fromAddress}, adding ${drops} to that address' unclaimed balance`)
        try {
          await this.addUnclaimedBalance(fromAddress, drops)
        } catch (err) { }
      } else {
        debug('Error crediting account: ', err)
        console.warn('Got incoming payment from an unknown account: ', JSON.stringify(tx))
      }
    }
  }
}
