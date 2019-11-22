local account = 'accounts:' .. ARGV[1]
local amount = tonumber(ARGV[2])
local idempotency_key = ARGV[3]

local balance, prepaid_amount = unpack(redis.call('HMGET', account, 'balance', 'prepaid_amount'))

-- If idempotency key has been used, then do not perform any operations
if redis.call('EXISTS', idempotency_key) == 1 then
    return balance + prepaid_amount
end

-- Otherwise, set it to true and make it expire after 24h (86400 sec)
redis.call('SET', idempotency_key, 'true', 'EX', 86400)

-- Credit the incoming settlement to the balance and/or prepaid amount,
-- depending on whether that account currently owes money or not
if tonumber(balance) >= 0 then
    prepaid_amount = redis.call('HINCRBY', account, 'prepaid_amount', amount)
elseif math.abs(balance) >= amount then
    balance = redis.call('HINCRBY', account, 'balance', amount)
else
    prepaid_amount = redis.call('HINCRBY', account, 'prepaid_amount', amount + balance)
    balance = 0
    redis.call('HSET', account, 'balance', 0)
end

return balance + prepaid_amount
