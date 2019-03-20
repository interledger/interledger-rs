import { RippleAPI } from 'ripple-lib'
import { RedisClient, createClient } from 'redis'
import { readFile } from 'fs'
import { promisify } from 'util'
import Debug from 'debug'
import { promisifyAll } from 'bluebird'
import * as path from 'path'
import { createHash } from 'crypto'
const debug = Debug('xrp-settlement-engine')
const readFileAsync = promisify(readFile)

// TODO should the settlement engine go through and make sure all of the xrp_addresses are stored in this hash map?
const DEFAULT_POLL_INTERVAL = 60000
const KEY_ARGS = 0

export interface XrpSettlementEngineConfig {
    address: string,
    secret: string,
    xrpServer?: string,
    redisUri?: string,
    minSettlementDrops?: number,
    pollInterval?: number
}

export class XrpSettlementEngine {
    private rippleClient: RippleAPI
    private redisClient: any
    private address: string
    private secret: string
    private minDropsToSettle: number
    private pollInterval: number
    private interval: NodeJS.Timeout
    private checkAccountsScript: string
    private creditAccountForSettlementScript: string
    private updateBalanceAfterSettlementScript: string

    constructor(config: XrpSettlementEngineConfig) {
        this.address = config.address
        this.secret = config.secret
        this.minDropsToSettle = (typeof config.minSettlementDrops === 'number' ? config.minSettlementDrops : 1000000)
        this.redisClient = promisifyAll(createClient(config.redisUri || 'redis://localhost:6379'))
        this.rippleClient = new RippleAPI({
            server: config.xrpServer || 'wss://s.altnet.rippletest.net:51233'// 'wss://s1.ripple.com'
        })
        this.pollInterval = config.pollInterval || DEFAULT_POLL_INTERVAL
    }

    async connect(): Promise<void> {
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
        this.rippleClient.connection.on('transaction', this.handleTransaction)
        await this.rippleClient.request('subscribe', {
            accounts: [this.address]
        })
    }

    private async loadScripts() {
        debug('Loading scripts into Redis')

        const checkAccountsScript = await readFileAsync(path.join(__dirname, "../scripts/get_accounts_that_need_settlement.lua"), 'utf8')
        await this.redisClient.scriptAsync('load', checkAccountsScript)
        this.checkAccountsScript = createHash('sha1').update(checkAccountsScript).digest('hex')

        const creditAccountForSettlementScript = await readFileAsync(path.join(__dirname, "../scripts/credit_account_for_settlement.lua"), 'utf8')
        await this.redisClient.scriptAsync('load', creditAccountForSettlementScript)
        this.creditAccountForSettlementScript = createHash('sha1').update(creditAccountForSettlementScript).digest('hex')

        const updateBalanceAfterSettlementScript = await readFileAsync(path.join(__dirname, "../scripts/update_balance_after_settlement.lua"), 'utf8')
        await this.redisClient.scriptAsync('load', creditAccountForSettlementScript)
        this.updateBalanceAfterSettlementScript = createHash('sha1').update(updateBalanceAfterSettlementScript).digest('hex')

        debug('Loaded scripts')
    }

    async disconnect(): Promise<void> {
        debug('Disconnecting')
        if (this.interval) {
            clearInterval(this.interval)
        }
        await Promise.all([
            this.rippleClient.disconnect().then(() => debug('Disconnected from rippled')),
            this.redisClient.disconnect().then(() => debug('Disconnected from Redis'))
        ])
        debug('Disconnected')
    }

    private async checkAccounts() {
        debug('Checking accounts')
        let cursor = '0'
        do {
            cursor = await this.scanAccounts(cursor)
        }
        while (cursor !== '0')
    }


    private async scanAccounts(cursor: string): Promise<string> {
        const [newCursor, accountsToSettle] = await this.redisClient.evalshaAsync(this.checkAccountsScript, KEY_ARGS, cursor, 20, this.minDropsToSettle)

        for (let accountRecord in accountsToSettle) {
            const [account, xrpAddress, dropsToSettle] = accountRecord
            this.settle(account, xrpAddress, dropsToSettle)
        }

        return newCursor
    }

    private async settle(account: string, xrpAddress: string, drops: string) {
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
                const newBalance = await this.redisClient.evalshaAsync(this.updateBalanceAfterSettlementScript, KEY_ARGS, account, drops)
                debug(`Account ${account} now has balance: ${newBalance}`)
            }
        } catch (err) {
            console.error(`Error preparing and submitting payment to rippled. Settlement to account: ${account} (xrpAddress: ${xrpAddress}) for ${drops} drops failed:`, err)
        }
    }

    private async handleTransaction(tx: any) {
        if (!tx.validated || tx.engine_result !== 'tesSUCCESS' || tx.transaction.TransactionType !== 'Payment' || tx.transaction.Destination !== this.address) {
            return
        }

        // Parse amount received from transaction
        let drops
        try {
            if (tx.meta.delivered_amount) {
                drops = parseInt(tx.meta.delivered_amount)
            } else {
                drops = parseInt(tx.transaction.Amount)
            }
        } catch (err) {
            console.error('Error parsing amount received from transaction: ', tx)
            return
        }

        try {
            const [account, newBalance] = await this.redisClient.evalshaAsync(this.creditAccountForSettlementScript, KEY_ARGS, tx.transaction.Account, drops)
            debug(`Got incoming settlement from account: ${account}, balance is now: ${newBalance}`)
        } catch (err) {
            debug('Error crediting account: ', err)
            console.warn('Got incoming payment from an unknown account: ', JSON.stringify(tx))
        }
    }
}