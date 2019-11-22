local from_id = ARGV[1]
local from_account = 'accounts:' .. ARGV[1]
local from_amount = tonumber(ARGV[2])
local min_balance, balance, prepaid_amount = unpack(redis.call('HMGET', from_account, 'min_balance', 'balance', 'prepaid_amount'))
balance = tonumber(balance)
prepaid_amount = tonumber(prepaid_amount)

-- Check that the prepare wouldn't go under the account's minimum balance
if min_balance then
    min_balance = tonumber(min_balance)
    if balance + prepaid_amount - from_amount < min_balance then
        error('Incoming prepare of ' .. from_amount .. ' would bring account ' .. from_id .. ' under its minimum balance. Current balance: ' .. balance .. ', min balance: ' .. min_balance)
    end
end

-- Deduct the from_amount from the prepaid_amount and/or the balance
if prepaid_amount >= from_amount then
    prepaid_amount = redis.call('HINCRBY', from_account, 'prepaid_amount', 0 - from_amount)
elseif prepaid_amount > 0 then
    local sub_from_balance = from_amount - prepaid_amount
    prepaid_amount = 0
    redis.call('HSET', from_account, 'prepaid_amount', 0)
    balance = redis.call('HINCRBY', from_account, 'balance', 0 - sub_from_balance)
else
    balance = redis.call('HINCRBY', from_account, 'balance', 0 - from_amount)
end

return balance + prepaid_amount
