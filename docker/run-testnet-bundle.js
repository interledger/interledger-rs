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
    runLocalTunnel(config.nodeName)
    runNode(config)

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
        console.log('Connecting to the Xpring Testnet node...')
        const testnetAuth = execSync(`ilp-cli testnet setup ${config.currency} \
            --auth=${config.adminAuthToken} \
            --return-testnet-credential`, {
            encoding: 'utf8'
        }).replace(/\s/g, '')

        // Configure the testnet node to talk to us over HTTP instead of BTP
        console.log('Setting up the nodes to use HTTP for bilateral communication...')
        const testnetUsername = testnetAuth.split(':')[0]
        const localUsername = `xpring_${config.currency.toLowerCase()}`
        // This changes the HTTP auth details so that both sides use the same auth token
        // It also requests a lower settlement threshold than the default so we see settlements going through
        execSync(`ilp-cli accounts update-settings ${localUsername} \
            --auth=${config.adminAuthToken} \
            --ilp-over-http-incoming-token=${config.xpringToken} \
            --ilp-over-http-outgoing-token=${testnetUsername}:${config.xpringToken} \
            --settle-threshold=1000 \
            --settle-to=0`)
        execSync(`ilp-cli --node=https://rs3.xpring.dev accounts update-settings ${testnetUsername} \
            --auth=${testnetAuth} \
            --ilp-over-http-incoming-token=${config.xpringToken} \
            --ilp-over-http-outgoing-token=${localUsername}:${config.xpringToken} \
            --ilp-over-http-url=https://${config.nodeName}.localtunnel.me/ilp \
            --settle-threshold=1000 \
            --settle-to=0`)
            // TODO: The outgoing token and HTTP url should be replaced with the new format
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
        const httpBindAddress = process.env.HTTP_BIND_ADDRESS || `0.0.0.0:7770`
        const adminAuthToken = process.env.ADMIN_AUTH_TOKEN || `admin-token-${randomBytes(20).toString('hex')}`
        const secretSeed = randomBytes(32).toString('hex')
        const currency = (process.env.CURRENCY || 'XRP').toUpperCase()
        const ethKey = process.env.ETH_SECRET_KEY || '380EB0F3D505F087E438ECA80BC4DF9A7FAA24F868E69FC0440261A0FC0567DC'
        const ethUrl = process.env.ETH_URL || 'https://rinkeby.infura.io/v3/2bd7fa2c387e4f07a5599bc64d0a9c33'
        const xpringToken = randomBytes(20).toString('hex')

        const config = {
            nodeName,
            httpBindAddress,
            adminAuthToken,
            secretSeed,
            currency,
            ethKey,
            ethUrl,
            xpringToken
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

function runNode({ httpBindAddress, adminAuthToken, secretSeed, nodeName }) {
    console.log('Starting ilp-node...')
    const node = spawn('ilp-node', [
        `--http_bind_address=${httpBindAddress}`,
        `--admin_auth_token=${adminAuthToken}`,
        `--secret_seed=${secretSeed}`,
        '--redis_url=redis+unix:/tmp/redis.sock',
        `--default_spsp_account=${nodeName}`
    ],
        {
            env: {
                'ILP_EXCHANGE_RATE__PROVIDER': 'CoinCap',
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
    const eth = spawn('ilp-settlement-ethereum', [
        '--settlement_api_bind_address=127.0.0.1:3002',
        `--ethereum_url=${ethUrl}`,
        `--private_key=${ethKey}`,
        '--poll_frequency=15000',
        '--confirmations=0',
        '--redis_url=redis+unix:/tmp/redis.sock?db=2',
        '--chain_id=4'
    ], {
        env: {
            'RUST_LOG': process.env.RUST_LOG || 'interledger=debug,ilp_settlement_ethereum=debug',
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
