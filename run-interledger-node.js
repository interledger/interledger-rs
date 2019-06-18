// NOTE: this script is designed to be run inside a docker container and may not work as intended outside of one

const { spawn } = require('child_process')
const { randomBytes } = require('crypto')
const https = require('https')
const fs = require('fs')
const { promisify } = require('util')

// Have the redis-server listen on a unix socket instead of TCP because it's faster
const REDIS_UNIX_SOCKET = '/tmp/redis.sock'
const XRP_FAUCET_URL = 'https://faucet.altnet.rippletest.net/accounts'
const XRP_TESTNET_URI = 'wss://s.altnet.rippletest.net:51233'
const CONFIG_PATH = '/data/node-config.json'

async function run() {
    let xrpAddress = process.env.XRP_ADDRESS
    let xrpSecret = process.env.XRP_SECRET
    let adminToken = process.env.ADMIN_TOKEN
    let rippled = process.env.XRP_SERVER
    let ilpAddress = process.env.ILP_ADDRESS
    const redisDir = process.env.REDIS_DIR || '.'

    let shouldCreateAdminAccount = true

    // Try reading from config file or generating testnet credentials
    if (!xrpAddress) {
        let config = await loadConfig()
        if (config) {
            shouldCreateAdminAccount = false
        } else {
            config = await generateTestnetCredentials()
        }
        xrpAddress = config.xrpAddress
        xrpSecret = config.xrpSecret
        adminToken = config.adminToken
        rippled = config.rippled
        ilpAddress = config.ilpAddress
    }

    if (!xrpAddress || !xrpSecret || !adminToken || !ilpAddress) {
        console.error('Must provide XRP_ADDRESS, XRP_SECRET, ILP_ADDRESS, and ADMIN_TOKEN')
        process.exit(1)
    }

    console.log('Starting XRP settlement engine')
    const settlementEngine = spawn('xrp-settlement-engine',
        [
            `--redis=${REDIS_UNIX_SOCKET}`,
            `--address=${xrpAddress}`,
            `--secret=${xrpSecret}`,
            `--rippled=${rippled}`
        ], {
            env: {
                DEBUG: process.env.DEBUG
            },
            stdio: 'inherit'
        })
    settlementEngine.on('error', (err) => console.error('Settlement engine error:', err))
    settlementEngine.on('exit', (code, signal) => console.error(`Settlement engine exited with code: ${code} and signal: ${signal}`))

    console.log('Creating admin account')
    const createAccount = spawn('interledger', [
        'node',
        'accounts',
        'add',
        `--redis_uri=unix:${REDIS_UNIX_SOCKET}`,
        `--ilp_address=${ilpAddress}`,
        `--xrp_address=${xrpAddress}`,
        `--http_incoming_token=${adminToken}`,
        '--asset_code=XRP',
        '--asset_scale=9',
        '--admin'
    ], {
            stdio: 'inherit',
            env: {
                RUST_LOG: process.env.RUST_LOG
            }
        })
    createAccount.on('error', (err) => console.error('Error creating account:', err))

    console.log('Launching Interledger node')
    const node = spawn('interledger', [
        'node',
        `--redis_uri=unix:${REDIS_UNIX_SOCKET}`,
    ], {
            stdio: 'inherit',
            env: {
                RUST_LOG: process.env.RUST_LOG,
            }
        })
    node.on('error', (err) => console.error('Interledger node error:', err))
    node.on('exit', (code, signal) => console.error(`Interledger node exited with code: ${code} and signal: ${signal}`))
}

run().catch((err) => console.error(err))

async function loadConfig() {
    try {
        const configFile = await promisify(fs.readFile)(CONFIG_PATH, { encoding: 'utf8' })
        const config = JSON.parse(config)
        return config
    } catch (err) {
        return null
    }
}

async function generateTestnetCredentials() {
    const adminToken = randomBytes(20).toString('hex')
    console.log(`Admin HTTP Bearer token is: ${adminToken}`)

    console.log('Fetching XRP testnet credentials')
    const faucetResponse = await new Promise((resolve, reject) => {
        let data = ''
        const req = https.request(XRP_FAUCET_URL, {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' }
        }, (res) => {
            res.on('data', (chunk) => {
                if (chunk) {
                    data += chunk
                }
            })
            res.on('end', (chunk) => {
                if (chunk) {
                    data += chunk
                }
                resolve(JSON.parse(data))
            })
        })
        req.once('error', reject)
        req.end()
    })
    const xrpAddress = faucetResponse.account.address
    const xrpSecret = faucetResponse.account.secret
    const rippled = XRP_TESTNET_URI
    const ilpAddress = `test.xrp.${xrpAddress}`
    console.log(`Got testnet XRP address: ${xrpAddress} and secret: ${xrpSecret}`)

    // Write config file
    try {
        await promisify(fs.writeFile)(CONFIG_PATH, JSON.stringify({
            xrpAddress,
            xrpSecret,
            adminToken,
            ilpAddress,
            rippled
        }))
    } catch (err) {
        console.error("Error writing to config file. XRP Address, Secret, and admin token were not saved.", err)
    }
    return {
        xrpAddress,
        xrpSecret,
        rippled,
        ilpAddress,
        adminToken
    }
}

