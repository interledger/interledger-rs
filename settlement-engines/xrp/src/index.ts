import { RippleAPI } from 'ripple-lib'
import { RedisClient, createClient } from 'redis'
// import { readFile } from 'fs'
import { promisify } from 'util'
import Debug from 'debug'
import { promisifyAll } from 'bluebird'
const debug = Debug('xrp-settlement-engine')

// TODO should the settlement engine go through and make sure all of the xrp_addresses are stored in this hash map?
const XRP_ADDRESSES_KEY = 'xrp_addresses'
const BALANCES_KEY = 'balances:xrp'
const ACCOUNTS_PREFIX = 'accounts:'

const DEFAULT_POLL_INTERVAL = 60000

export interface XrpSettlementEngineConfig {
    address: string,
    secret: string,
    xrpServer?: string,
    redisUri?: string,
    scale?: number,
    minSettlementDrops?: number,
    pollInterval?: number
}

export class XrpSettlementEngine {
    private rippleClient: RippleAPI
    private redisClient: any
    private address: string
    private secret: string
    private scale: number
    private minSettlementAmount: number
    private pollInterval: number
    private interval: NodeJS.Timeout

    constructor(config: XrpSettlementEngineConfig) {
        this.address = config.address
        this.secret = config.secret
        this.scale = (typeof config.scale === 'number' ? config.scale : 9)
        this.minSettlementAmount = this.xrpDropsToAmount(typeof config.minSettlementDrops === 'number' ? config.minSettlementDrops : 1000000)
        this.redisClient = promisifyAll(createClient(config.redisUri || 'redis://localhost:6379'))
        this.rippleClient = new RippleAPI({
            server: config.xrpServer || 'wss://s.altnet.rippletest.net:51233'// 'wss://s1.ripple.com'
        })
        this.pollInterval = config.pollInterval || DEFAULT_POLL_INTERVAL
    }

    async connect(): Promise<void> {
        await Promise.all([
            this.rippleClient.connect().then(() => debug('Connected to rippled')),
            new Promise((resolve, reject) => {
                this.redisClient.once('ready', resolve)
                this.redisClient.once('error', reject)
            }).then(() => debug('Connected to Redis'))
        ])

        await this.checkAccounts()
        this.interval = setInterval(() => this.checkAccounts(), this.pollInterval)

        // Load and register script
        // const checkBalancesScript = await readFileAsync("./check-balances.lua", 'utf8')
        // this.redisClient.script('load', checkBalancesScript)

        this.rippleClient.connection.on('transaction', this.handleTransaction)

        await this.rippleClient.request('subscribe', {
            accounts: [this.address]
        })
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
        debug('Scanning accounts')
        // TODO do the scan and lookup of the associated details in a lua script
        const result = await this.redisClient.hscanAsync(BALANCES_KEY, cursor)
        const newCursor = result[0]
        const balances = result[1]
        let command = this.redisClient.multi()
        for (let i = 0; i < balances.length; i += 2) {
            const account = balances[i]
            const balance = balances[i + 1]
            command = command.hmget(`${ACCOUNTS_PREFIX}${account}`, ['settle_threshold', 'settle_to', 'xrp_address'])
            debug(`Account ${account} has balance: ${balance}`)
        }
        promisify(command.exec).call(command)
            .then((results) => {
                debug('Got account details:', results)
                for (let i = 0; i < results.length; i++) {
                    const account = balances[i * 2]
                    const balance = parseInt(balances[i * 2 + 1])
                    const settleThreshold = parseInt(results[i][0])
                    const settleTo = parseInt(results[i][1])
                    const xrpAddress = results[i][2]
                    debug(`Account ${account} has balance: ${balance}, settleThreshold: ${settleThreshold}, and settleTo: ${settleTo}`)
                    if (balance >= settleThreshold && balance - settleTo >= this.minSettlementAmount) {
                        const amountToSettle = balance - settleTo
                        this.settle(account, xrpAddress, amountToSettle)
                    }
                }
            })
            .catch((err) => console.error('Error getting account details:', err))
        return newCursor
    }

    private async settle(account: string, xrpAddress: string, amount: number) {
        const drops = this.amountToXrpDrops(amount)
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
                const adjustmentAmount = 0 - this.xrpDropsToAmount(drops)
                const newBalance = await this.redisClient.hincrbyAsync(BALANCES_KEY, account, adjustmentAmount)
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
        const amount = this.xrpDropsToAmount(drops)

        // TODO look up the account and do the set transaction in lua
        const account = await this.redisClient.hgetAsync(XRP_ADDRESSES_KEY, tx.transaction.Account)
        if (account) {
            const newBalance = await this.redisClient.hincrbyAsync(BALANCES_KEY, account, amount)
            debug(`Got incoming settlement from account: ${account}, balance is now: ${newBalance}`)
        } else {
            console.warn('Got incoming payment from an unknown account: ', JSON.stringify(tx))
        }
    }

    private amountToXrpDrops(amount: number): number {
        return Math.floor(amount * (Math.pow(10, 6 - this.scale)))
    }

    private xrpDropsToAmount(drops: number): number {
        return Math.floor(drops * Math.pow(10, this.scale - 6))
    }
}