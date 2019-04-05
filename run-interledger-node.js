// NOTE: this script is designed to be run inside a docker container and may not work as intended outside of one

const { spawn } = require('child_process')
const { randomBytes } = require('crypto')
const https = require('https')
const fs = require('fs')
const { promisify } = require('util')
const localtunnel = require('localtunnel')

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
    let useLocaltunnel = !process.env.DISABLE_LOCALTUNNEL
    let localTunnelSubdomain = process.env.LOCALTUNNEL_SUBDOMAIN
    const redisDir = process.env.REDIS_DIR || '.'

    let createAdminAccount = true

    // Try reading from config file or generating testnet credentials
    if (!xrpAddress) {
        let config = await loadConfig()
        createAdminAccount = false
        if (!config) {
            config = await generateTestnetCredentials()
        }
        xrpAddress = config.xrpAddress
        xrpSecret = config.xrpSecret
        adminToken = config.adminToken
        rippled = config.rippled
        ilpAddress = config.ilpAddress
        localTunnelSubdomain = localTunnelSubdomain || config.localTunnelSubdomain
    }

    if (!xrpAddress || !xrpSecret || !adminToken || !ilpAddress) {
        console.error('Must provide XRP_ADDRESS, XRP_SECRET, ILP_ADDRESS, and ADMIN_TOKEN')
        process.exit(1)
    }

    console.log('Starting redis-server')
    const redis = spawn('redis-server', [
        // Use a unix socket instead of TCP
        `--unixsocket ${REDIS_UNIX_SOCKET}`,
        '--unixsocketperm 777',
        // Save redis data using append-only log of commands
        '--appendonly yes',
        '--appendfsync everysec',
        `--dir ${redisDir}`
    ], {
            stdio: 'inherit'
        })
    redis.on('error', (err) => console.error('Redis error:', err))
    redis.on('exit', (code, signal) => console.error(`Redis exited with code: ${code} and signal: ${signal}`))

    if (useLocaltunnel) {
        await runLocalTunnel(localTunnelSubdomain, true)
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

    if (createAdminAccount) {
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

        // Wait for the default account to be created before launching the node
        await new Promise((resolve) => {
            createAccount.once('exit', resolve)
            setTimeout(resolve, 1000)
        })
    }

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

    await new Promise((resolve) => setTimeout(resolve, 200))

    console.log('\n\n')

    // Print instructions
    if (!process.env.ADMIN_TOKEN) {
        console.log(`>>> Admin API Authorization header: "Bearer ${adminToken}"\n`)
    }

    if (!process.env.LOCALTUNNEL_SUBDOMAIN) {
        console.log(`>>> Node is accessible via: https://${localTunnelSubdomain}.localtunnel.me \n`)
    }

    if (!process.env.XRP_ADDRESS) {
        console.log(`>>> XRP Address: ${xrpAddress} \n`)
    }

    console.log('\n')
}

run().catch((err) => console.error(err))

async function loadConfig() {
    try {
        console.log('Loading config...')
        const configFile = await promisify(fs.readFile).call(null, CONFIG_PATH, { encoding: 'utf8' })
        const config = JSON.parse(configFile)
        console.log('Loaded config')
        return config
    } catch (err) {
        console.log('No config loaded')
        return null
    }
}

async function generateTestnetCredentials() {
    console.log('Generating testnet credentials...')
    const adminToken = randomBytes(20).toString('hex')
    const localTunnelSubdomain = 'ilp-node-' + randomBytes(12).toString('hex')

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
        await promisify(fs.writeFile).call(null, CONFIG_PATH, JSON.stringify({
            xrpAddress,
            xrpSecret,
            adminToken,
            ilpAddress,
            rippled,
            localTunnelSubdomain
        }))
        console.log('Saved config to ', CONFIG_PATH)
    } catch (err) {
        console.error("Error writing to config file. XRP Address, Secret, and admin token were not saved.", err)
    }
    return {
        xrpAddress,
        xrpSecret,
        rippled,
        ilpAddress,
        adminToken,
        localTunnelSubdomain
    }
}

async function runLocalTunnel(subdomain, restartOnError) {
    console.log('Starting localtunnel to expose the local node to others')

    const tunnel = await promisify(localtunnel).call(null, 7770, { subdomain })

    tunnel.on('close', function () {
        console.error('localtunnel closed')
        if (restartOnError) {
            runLocalTunnel(subdomain, restartOnError)
        }
    })
}
