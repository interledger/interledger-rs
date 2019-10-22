local account = 'accounts:' .. ARGV[1]
local settle_amount = tonumber(ARGV[2])

local balance = redis.call('HINCRBY', account, 'balance', settle_amount)
return balance
