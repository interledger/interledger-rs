// NOTE: this script is designed to be run inside a docker container and may not work as intended outside of one

const { spawn } = require('child_process')
const { randomBytes } = require('crypto')
const fs = require('fs')
const { promisify } = require('util')
const localtunnel = require('localtunnel')
const request = require('request-promise-native')

// Have the redis-server listen on a unix socket instead of TCP because it's faster
const REDIS_UNIX_SOCKET = '/tmp/redis.sock'
const CONFIG_PATH = '/data/node-config.json'

async function run() {
    let useLocaltunnel = !process.env.DISABLE_LOCALTUNNEL
    let localTunnelSubdomain = process.env.LOCALTUNNEL_SUBDOMAIN
    const redisDir = process.env.REDIS_DIR || '.'
    let adminToken = process.env.ILP_ADMIN_AUTH_TOKEN
    let ilpAddress = process.env.ILP_ADDRESS
    let secretSeed = process.env.ILP_SECRET_SEED

    let createAccounts = false

    // Try reading from config file or generating credentials if they weren't passed in
    if (!adminToken || !ilpAddress || !secretSeed) {
        let config = await loadConfig()
        if (!config) {
            config = await generateTestnetCredentials()
            createAccounts = true
        }
        adminToken = config.adminToken
        ilpAddress = config.ilpAddress
        localTunnelSubdomain = config.localTunnelSubdomain
        secretSeed = config.secretSeed
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

    // Copy in env variables
    const env = {
        ILP_ADDRESS: ilpAddress,
        ILP_REDIS_CONNECTION: `unix:${REDIS_UNIX_SOCKET}`,
        ILP_ADMIN_AUTH_TOKEN: adminToken,
        ILP_SECRET_SEED: secretSeed,
        ILP_DEFAULT_SPSP_ACCOUNT: 0,
        RUST_LOG: 'interledger/.*'
    }
    for (let key in process.env) {
        if (key.startsWith('RUST') || key.startsWith('ILP')) {
            env[key] = process.env[key]
        }
    }
    console.log('Launching Interledger node')
    const node = spawn('interledger', [ 'node', ], {
            stdio: 'inherit',
            env
        })
    node.on('error', (err) => console.error('Interledger node error:', err))
    node.on('exit', (code, signal) => console.error(`Interledger node exited with code: ${code} and signal: ${signal}`))

    await new Promise((resolve) => setTimeout(resolve, 500))

    if (createAccounts) {
        // Create node operator account
        console.log('Creating account for node operator')
        try {
            await request({
                method: 'POST',
                uri: 'http://localhost:7770/accounts',
                headers: {
                    Authorization: `Bearer ${adminToken}`
                },
                json: true,
                body: {
                    ilp_address: ilpAddress,
                    http_incoming_token: adminToken,
                    asset_code: 'XRP',
                    asset_scale: 9
                }
            })
        } catch (err) {
            throw new Error(`Unable to create node operator account: ${err.message}`)
        }
        console.log('Created operator account')
    }

    console.log('\n\n')

    // Print instructions
    const nodeDomain = useLocaltunnel ? `https://${localTunnelSubdomain}.localtunnel.me` : 'http://172.17.0.2:7770'
    console.log(`>>> Node is accessible on: ${nodeDomain} \n`)

    if (!process.env.ADMIN_TOKEN) {
        console.log(`>>> Admin API Authorization header: "Bearer ${adminToken}"\n`)
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
    const adminToken = randomBytes(32).toString('hex')
    const nodeId = randomBytes(20).toString('hex')
    const localTunnelSubdomain = `ilp-node-${nodeId}`
    const ilpAddress = `test.${nodeId}`
    const secretSeed = randomBytes(32).toString('hex')

    // Write config file
    try {
        await promisify(fs.writeFile).call(null, CONFIG_PATH, JSON.stringify({
            adminToken,
            ilpAddress,
            localTunnelSubdomain,
            secretSeed
        }))
        console.log('Saved config to ', CONFIG_PATH)
    } catch (err) {
        console.error("Error writing to config file, configuration was not saved:", err)
        process.exit(1)
    }
    return {
        ilpAddress,
        adminToken,
        localTunnelSubdomain,
        secretSeed
    }
}

async function runLocalTunnel(subdomain, restartOnError) {
    if (!subdomain) {
        throw new Error('Cannot start localtunnel with undefined subdomain')
    }
    console.log(`Starting localtunnel and requesting subdomain ${subdomain}`)

    const tunnel = await promisify(localtunnel).call(null, 7770, { subdomain })

    tunnel.on('error', (err) => console.error('localtunnel error:', err))

    tunnel.on('close', function () {
        console.error('localtunnel closed')
        if (restartOnError) {
            runLocalTunnel(subdomain, restartOnError)
        }
    })
}
