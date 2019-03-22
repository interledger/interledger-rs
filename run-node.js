const { spawn } = require('child_process')

// Have the redis-server listen on a unix socket instead of TCP because it's faster
const REDIS_UNIX_SOCKET = '/tmp/redis.sock'

async function run() {
    const xrpAddress = process.env.XRP_ADDRESS
    const xrpSecret = process.env.XRP_SECRET
    const adminToken = process.env.ADMIN_TOKEN
    const ilpAddress = process.env.ILP_ADDRESS || 'private.local.node'

    if (!xrpAddress || !xrpSecret || !adminToken) {
        console.error('Must provide XRP_ADDRESS, XRP_SECRET, and ADMIN_TOKEN')
        process.exit(1)
    }

    console.log('Starting redis-server')
    const redis = spawn('redis-server', [
        `--unixsocket ${REDIS_UNIX_SOCKET}`,
        '--unixsocketperm 777'
    ], {
            stdio: 'inherit'
        })
    redis.on('error', (err) => console.error('Redis error:', err))
    redis.on('exit', (code, signal) => console.error(`Redis exited with code: ${code} and signal: ${signal}`))

    console.log('Starting XRP settlement engine')
    const settlementEngine = spawn('./settlement-engines/xrp/build/cli.js', [`--address=${xrpAddress}`, `--secret=${xrpSecret}`], {
        env: {
            DEBUG: process.env.DEBUG
        },
        stdio: 'inherit'
    })
    settlementEngine.on('error', (err) => console.error('Settlement engine error:', err))
    settlementEngine.on('exit', (code, signal) => console.error(`Settlement engine exited with code: ${code} and signal: ${signal}`))

    console.log('Creating admin account')
    const createAccount = spawn('./target/debug/interledger', [
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
    const node = spawn('./target/debug/interledger', [
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

