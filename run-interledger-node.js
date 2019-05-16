// NOTE: this script is designed to be run inside a docker container and may not work as intended outside of one

const { spawn } = require('child_process')
const { randomBytes } = require('crypto')
const fs = require('fs')
const { promisify } = require('util')
const localtunnel = require('localtunnel')

// Have the redis-server listen on a unix socket instead of TCP because it's faster
const REDIS_UNIX_SOCKET = '/tmp/redis.sock'
const CONFIG_PATH = '/data/node-config.json'

async function run() {
    let adminToken = process.env.ADMIN_TOKEN
    let ilpAddress = process.env.ILP_ADDRESS
    let useLocaltunnel = !process.env.DISABLE_LOCALTUNNEL
    let localTunnelSubdomain = process.env.LOCALTUNNEL_SUBDOMAIN
    const redisDir = process.env.REDIS_DIR || '.'
    let nodeDomain = process.env.DOMAIN
    let nodeSecret = process.env.ILP_NODE_SECRET

    let createAccounts = true

    // Try reading from config file or generating testnet credentials
    if (!adminToken) {
        let config = await loadConfig()
        if (config) {
            createAccounts = false
        } else {
            config = await generateTestnetCredentials()
        }
        adminToken = config.adminToken
        ilpAddress = config.ilpAddress
        localTunnelSubdomain = localTunnelSubdomain || config.localTunnelSubdomain
        nodeSecret = nodeSecret || config.nodeSecret
    }

    if (!nodeDomain) {
        if (useLocaltunnel) {
            nodeDomain = `https://${localTunnelSubdomain}.localtunnel.me`
        } else {
            nodeDomain = 'http://172.17.0.2:7770'
        }
    }

    if (!adminToken || !ilpAddress) {
        console.error('Must provide ILP_ADDRESS and ADMIN_TOKEN')
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

    console.log('Launching Interledger node')
    const node = spawn('interledger', [
        'node',
        `--redis_uri=unix:${REDIS_UNIX_SOCKET}`,
        `--server_secret=${nodeSecret}`,
    ], {
            stdio: 'inherit',
            env: {
                RUST_LOG: process.env.RUST_LOG,
            }
        })
    node.on('error', (err) => console.error('Interledger node error:', err))
    node.on('exit', (code, signal) => console.error(`Interledger node exited with code: ${code} and signal: ${signal} `))

    await new Promise((resolve) => setTimeout(resolve, 200))

    console.log('\n\n')

    // Print instructions
    console.log(`>>> Node is accessible on: ${nodeDomain} \n`)

    if (!process.env.ADMIN_TOKEN) {
        console.log(`>>> Admin API Authorization header: "Bearer ${adminToken}"\n`)
    }

    console.log(`>>> Try sending a test payment by sending an HTTP POST request to:

    ${nodeDomain}/pay

    with the header: "Authorization": "Bearer ${adminToken}"
    and the body:
    {
        "receiver": "$receiver-payment-pointer.example",
        "source_amount": 1000
    }
    `)

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
    const ilpAddress = `test.${randomBytes(20).toString('hex')} `
    const nodeSecret = randomBytes(32).toString('hex')

    // Write config file
    try {
        await promisify(fs.writeFile).call(null, CONFIG_PATH, JSON.stringify({
            adminToken,
            ilpAddress,
            localTunnelSubdomain,
            nodeSecret
        }))
        console.log('Saved config to ', CONFIG_PATH)
    } catch (err) {
        console.error("Error writing to config file. XRP Address, Secret, and admin token were not saved.", err)
    }
    return {
        ilpAddress,
        adminToken,
        localTunnelSubdomain,
        nodeSecret
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
