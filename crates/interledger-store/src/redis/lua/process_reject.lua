local accounts_key = ARGV[1]
local from_account = accounts_key .. ':' .. ARGV[2]
local from_amount = tonumber(ARGV[3])

local prepaid_amount = redis.call('HGET', from_account, 'prepaid_amount')
local balance = redis.call('HINCRBY', from_account, 'balance', from_amount)
return balance + prepaid_amount
