local account = 'accounts:' .. ARGV[1]
local requested_amount = tonumber(ARGV[2])

local min_balance, balance, prepaid_amount = unpack(redis.call('HMGET', account, 'min_balance', 'balance', 'prepaid_amount'))
balance = tonumber(balance)
prepaid_amount = tonumber(prepaid_amount)

-- check that the account's balance is not negative
if balance + prepaid_amount < 0  then
    error('Account has negative balance')
end

-- Check that the account's balance will not become negative
if balance + prepaid_amount - requested_amount < 0 then
    error('Withdrawal of ' .. requested_amount .. ' would bring account ' .. account .. ' under 0 balance. Current balance: ' .. balance .. ', min balance: ' .. min_balance)
end

-- Check that the prepare wouldn't go under the account's minimum balance
if min_balance then
    min_balance = tonumber(min_balance)
    if balance + prepaid_amount - requested_amount < min_balance then
        error('Withdrawal of ' .. requested_amount .. ' would bring account ' .. account .. ' under its minimum balance. Current balance: ' .. balance .. ', min balance: ' .. min_balance)
    end
end

-- Deduct the requested amount from the prepaid_amount and/or the balance
if prepaid_amount >= requested_amount then
    prepaid_amount = redis.call('HINCRBY', account, 'prepaid_amount', 0 - requested_amount)
elseif prepaid_amount > 0 then
    local sub_from_balance = requested_amount - prepaid_amount
    prepaid_amount = 0
    redis.call('HSET', account, 'prepaid_amount', 0)
    balance = redis.call('HINCRBY', account, 'balance', 0 - sub_from_balance)
else
    balance = redis.call('HINCRBY', account, 'balance', 0 - requested_amount)
end

-- return the updated balance
return balance + prepaid_amount