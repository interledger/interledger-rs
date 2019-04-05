// NOTE: this script is designed to be run inside a docker container and may not work as intended outside of one

const { spawn } = require('child_process')
const { randomBytes } = require('crypto')
const fs = require('fs')
const { promisify } = require('util')
const localtunnel = require('localtunnel')
const request = require('request-promise-native')

// Have the redis-server listen on a unix socket instead of TCP because it's faster
const REDIS_UNIX_SOCKET = '/tmp/redis.sock'
const XRP_FAUCET_URL = 'https://faucet.altnet.rippletest.net/accounts'
const XRP_TESTNET_URI = 'wss://s.altnet.rippletest.net:51233'
const CONFIG_PATH = '/data/node-config.json'

const BOOTSTRAP_NODES = [{
    signupEndpoint: 'https://ilp-node-2204f3bf5901558b1ef045f6.localtunnel.me/accounts/prepaid',
    ilpEndpoint: 'https://ilp-node-2204f3bf5901558b1ef045f6.localtunnel.me/ilp',
    xrpAddress: 'rQ3eSt2HyozxRixDUxFanCnsw79zE1np1x',
    ilpAddress: 'test.xrp.rQ3eSt2HyozxRixDUxFanCnsw79zE1np1x'
}]

async function run() {
    let xrpAddress = process.env.XRP_ADDRESS
    let xrpSecret = process.env.XRP_SECRET
    let adminToken = process.env.ADMIN_TOKEN
    let rippled = process.env.XRP_SERVER
    let ilpAddress = process.env.ILP_ADDRESS
    let useLocaltunnel = !process.env.DISABLE_LOCALTUNNEL
    let localTunnelSubdomain = process.env.LOCALTUNNEL_SUBDOMAIN
    const redisDir = process.env.REDIS_DIR || '.'
    let nodeDomain = process.env.DOMAIN
    const connectToBootstrapNodes = !process.env.DISABLE_BOOTSTRAP

    let createAccounts = true

    // Try reading from config file or generating testnet credentials
    if (!xrpAddress) {
        let config = await loadConfig()
        if (config) {
            createAccounts = false
        } else {
            config = await generateTestnetCredentials()
        }
        xrpAddress = config.xrpAddress
        xrpSecret = config.xrpSecret
        adminToken = config.adminToken
        rippled = config.rippled
        ilpAddress = config.ilpAddress
        localTunnelSubdomain = localTunnelSubdomain || config.localTunnelSubdomain
    }

    if (!nodeDomain) {
        if (useLocaltunnel) {
            nodeDomain = `https://${localTunnelSubdomain}.localtunnel.me`
        } else {
            nodeDomain = 'http://172.17.0.2:7770'
        }
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
            `--rippled=${rippled}`,
            `--poll_interval=15000`
        ], {
            env: {
                DEBUG: process.env.DEBUG
            },
            stdio: 'inherit'
        })
    settlementEngine.on('error', (err) => console.error('Settlement engine error:', err))
    settlementEngine.on('exit', (code, signal) => console.error(`Settlement engine exited with code: ${code} and signal: ${signal}`))

    if (createAccounts) {
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
            '--admin',
            // TODO make this unlimited
            '--min_balance=-1000000000'
        ], {
                stdio: 'inherit',
                env: {
                    RUST_LOG: process.env.RUST_LOG
                }
            })
        createAccount.on('error', (err) => console.error('Error creating account:', err))

        if (connectToBootstrapNodes) {
            console.log('Peering with bootstrap nodes')
            for (let node of BOOTSTRAP_NODES) {
                const authToken = randomBytes(20).toString('hex')
                const peerAuthToken = randomBytes(20).toString('hex')
                node.authToken = authToken
                node.peerAuthToken = peerAuthToken
                const httpUrl = node.ilpEndpoint.replace('//', `//:${node.authToken}@`)
                console.log(httpUrl)
                const createAccount = spawn('interledger', [
                    'node',
                    'accounts',
                    'add',
                    `--redis_uri=unix:${REDIS_UNIX_SOCKET}`,
                    `--ilp_address=${node.ilpAddress}`,
                    `--xrp_address=${node.xrpAddress}`,
                    `--http_incoming_token=${node.peerAuthToken}`,
                    `--http_url=${httpUrl}`,
                    '--asset_code=XRP',
                    '--asset_scale=9',
                    '--settle_threshold=0',
                    '--settle_to=-1000000000',
                    '--min_balance=-10000000000',
                    '--send_routes',
                    '--receive_routes'
                ], {
                        stdio: 'inherit',
                        env: {
                            RUST_LOG: process.env.RUST_LOG
                        }
                    })
                createAccount.on('error', (err) => console.error('Error creating peer account:', err))
            }

            // Wait to make sure the settlement engine has sent the outgoing payment
            await new Promise((resolve) => {
                createAccount.once('exit', resolve)
                setTimeout(resolve, 20000)
            })

            for (let node of BOOTSTRAP_NODES) {
                console.log(`Creating account on bootstrap node: ${node.signupEndpoint}`)
                await request.post({
                    uri: node.signupEndpoint,
                    body: {

                        ilp_address: ilpAddress,
                        xrp_address: xrpAddress,
                        asset_code: 'XRP',
                        asset_scale: 9,
                        http_incoming_authorization: `Bearer ${node.authToken}`,
                        http_outgoing_authorization: `Bearer ${node.peerAuthToken}`,
                        http_endpoint: nodeDomain,
                        settleThreshold: 100000000,
                        settleTo: 0,
                        send_routes: true,
                        receive_routes: true
                    },
                    json: true
                })
            }
        } else {
            // Wait for the default account to be created before launching the node
            await new Promise((resolve) => {
                createAccount.once('exit', resolve)
                setTimeout(resolve, 1000)
            })
        }
    }

    console.log('Launching Interledger node')
    const node = spawn('interledger', [
        'node',
        `--redis_uri=unix:${REDIS_UNIX_SOCKET}`,
        // TODO make this greater than 0
        '--open_signups_min_balance=0'
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

    if (!process.env.XRP_ADDRESS) {
        console.log(`>>> XRP Address: ${xrpAddress} \n`)
    }

    console.log(`>>> Try sending a test payment by sending an HTTP POST request to:

    ${nodeDomain}/pay

    with the header: "Authorization": "Bearer ${adminToken}"
    and the body:
    {
        "receiver": "$ilp-node-2204f3bf5901558b1ef045f6.localtunnel.me",
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

    console.log('Fetching XRP testnet credentials')
    const faucetResponse = await request.post(XRP_FAUCET_URL, { json: true })
    const xrpAddress = faucetResponse.account.address
    const xrpSecret = faucetResponse.account.secret
    const rippled = XRP_TESTNET_URI
    const ilpAddress = `test.xrp.${xrpAddress} `
    console.log(`Got testnet XRP address: ${xrpAddress} and secret: ${xrpSecret} `)

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
