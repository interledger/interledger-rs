local from_account = 'accounts:' .. ARGV[1]
local from_amount = tonumber(ARGV[2])

local prepaid_amount = redis.call('HGET', from_account, 'prepaid_amount')
local balance = redis.call('HINCRBY', from_account, 'balance', from_amount)
return balance + prepaid_amount
