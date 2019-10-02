// NOTE: this script is designed to be run inside a docker container and may not work as intended outside of one

const { spawn, execSync } = require('child_process')
const { randomBytes } = require('crypto')
const fs = require('fs')

const configFilePath = "/data/config.json"

let restartLocalTunnel = true
// Don't restart localtunnel if the user kills the container
process.on('SIGINT', () => restartLocalTunnel = false)
process.on('SIGHUP', () => restartLocalTunnel = false)
process.on('exit', () => restartLocalTunnel = false)

async function run() {
    let config = loadConfig()

    runRedis()
    runNode(config)
    runLocalTunnel(config.nodeName)

    // if (config.currency === 'XRP') {
    runXrpSettlementEngine()
    // } else if (config.currency === 'ETH') {
    runEthSettlementEngine(config)
    // }

    if (config.firstRun) {
        await new Promise((resolve) => setTimeout(resolve, 1000))
        console.log(`Configuring node to connect to testnet using ${config.currency}...`)

        // Create the main account we will use for sending and receiving
        console.log(`Creating account: ${config.nodeName}`)
        execSync(`ilp-cli accounts create ${config.nodeName} \
            --auth=${config.adminAuthToken} \
            --asset-code=${config.currency} \
            --asset-scale=9 \
            --ilp-over-http-incoming-token=${config.nodeName}`)
        // Configure the node to use the settlement engines running in this container
        execSync(`ilp-cli settlement-engines set-all \
            --auth=${config.adminAuthToken} \
            --pair XRP http://localhost:3001 \
            --pair ETH http://localhost:3002`)
        // Connect to the Xpring Testnet node so we're connected to the rest of the testnet
        execSync(`ilp-cli testnet setup ${config.currency} --auth=${config.adminAuthToken}`)

        const xpringToken = randomBytes(32).toString('hex')
        // execSync(`ilp-cli --node=https://rs3.xpring.dev update-settings \
        //     --auth=${}
        //     --ilp-over-http-outgoing-token=${xpringToken} \
        //     --ilp-over-http-url=https://${config.nodeName}.localtunnel.me/ilp`)

    }

    console.log('Using config: ', config)
}

function loadConfig() {
    if (fs.existsSync(configFilePath)) {
        const config = JSON.parse(fs.readFileSync(configFilePath))
        config.firstRun = false
        return config
    } else {
        const nodeName = process.env.NAME || `ilp_node_${randomBytes(10).toString('hex')}`
        const adminAuthToken = `admin-token-${randomBytes(32).toString('hex')}`
        const secretSeed = randomBytes(32).toString('hex')
        const currency = (process.env.CURRENCY || 'XRP').toUpperCase()
        const ethKey = process.env.ETH_SECRET_KEY || '380EB0F3D505F087E438ECA80BC4DF9A7FAA24F868E69FC0440261A0FC0567DC'
        const ethUrl = process.env.ETH_URL || 'https://rinkeby.infura.io/v3/516b56b980c64f799942937106cdb200'

        const config = {
            nodeName,
            adminAuthToken,
            secretSeed,
            currency,
            ethKey,
            ethUrl
        }
        fs.writeFileSync(configFilePath, JSON.stringify(config))

        config.firstRun = true
        return config
    }
}

function runRedis() {
    console.log('Starting redis-server...')
    const redis = spawn('redis-server', [
        // Use a unix socket instead of TCP
        `--unixsocket /tmp/redis.sock`,
        '--unixsocketperm 777',
        // Save redis data using append-only log of commands
        '--appendonly yes',
        '--appendfsync everysec',
        `--dir /data`
    ], {
        stdio: 'inherit'
    })
    redis.on('error', (err) => console.error('Redis error:', err))
    redis.on('exit', (code, signal) => console.error(`Redis exited with code: ${code} and signal: ${signal}`))
    return redis
}

function runNode({ adminAuthToken, secretSeed }) {
    console.log('Starting ilp-node...')
    const node = spawn('ilp-node', [
        `--admin_auth_token=${adminAuthToken}`,
        `--secret_seed=${secretSeed}`,
        '--redis_url=unix:/tmp/redis.sock',
        // `--default_spsp_account=${nodeName}`
    ],
        {
            env: {
                'ILP_EXCHANGE_RATE_PROVIDER': 'coin_cap',
                'RUST_LOG': process.env.RUST_LOG || 'interledger=debug',
                'RUST_BACKTRACE': process.env.RUST_BACKTRACE || '1'
            },
            stdio: 'inherit'
        })
    node.on('error', (err) => console.error('ilp-node error:', err))
    node.on('exit', (code, signal) => console.error(`ilp-node exited with code: ${code} and signal: ${signal}`))
    return node
}

function runXrpSettlementEngine() {
    console.log('Starting ilp-settlement-xrp...')
    const xrp = spawn('ilp-settlement-xrp', {
        env: {
            // TODO this should pass in an XRP secret so it doesn't use a new account each time
            'DEBUG': 'settlement*',
            'REDIS_URI': 'redis://127.0.0.1:6379/1',
            'ENGINE_PORT': 3001
        },
        stdio: 'inherit'
    })
    xrp.on('error', (err) => console.error('ilp-setlement-xrp error:', err))
    xrp.on('exit', (code, signal) => console.error(`ilp-settlement-xrp exited with code: ${code} and signal: ${signal}`))
    return xrp
}

function runEthSettlementEngine({ ethKey, ethUrl }) {
    console.log('Starting ETH settlement engine...')
    const eth = spawn('interledger-settlement-engines', [
        'ethereum-ledger',
        '--settlement_api_bind_address=127.0.0.1:3002',
        `--ethereum_url=${ethUrl}`,
        `--private_key=${ethKey}`,
        '--redis_url=unix:/tmp/redis.sock?db=2',
        '--chain_id=4'
    ], {
        env: {
            'RUST_LOG': process.env.RUST_LOG || 'interledger=debug',
            'RUST_BACKTRACE': process.env.RUST_BACKTRACE || '1'
        },
        stdio: 'inherit'
    })
}

function runLocalTunnel(subdomain) {
    if (!subdomain) {
        throw new Error('Cannot start localtunnel with undefined subdomain')
    }
    console.log(`Starting localtunnel and requesting subdomain ${subdomain}`)
    const tunnel = spawn('lt', [
        '--port=7770',
        `--subdomain=${subdomain}`
    ], {
        stdio: 'inherit'
    })

    tunnel.on('error', (err) => console.error('localtunnel error:', err))
    tunnel.on('close', function () {
        console.error('localtunnel closed')
        if (restartLocalTunnel) {
            runLocalTunnel(subdomain)
        }
    })
}

run().catch(err => console.log(err))
