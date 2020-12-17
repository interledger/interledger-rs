local accounts_key = ARGV[1]
local account = accounts_key .. ':' .. ARGV[2]
local settle_amount = tonumber(ARGV[3])

local balance = redis.call('HINCRBY', account, 'balance', settle_amount)
return balance