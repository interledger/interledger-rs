local to_account = 'accounts:' .. ARGV[1]
local to_amount = tonumber(ARGV[2])

local balance = redis.call('HINCRBY', to_account, 'balance', to_amount)
local prepaid_amount, settle_threshold, settle_to = unpack(redis.call('HMGET', to_account, 'prepaid_amount', 'settle_threshold', 'settle_to'))

-- The logic for trigerring settlement is as follows:
--  1. settle_threshold must be non-nil (if it's nil, then settlement was perhaps disabled on the account).
--  2. balance must be greater than settle_threshold (this is the core of the 'should I settle logic')
--  3. settle_threshold must be greater than settle_to (e.g., settleTo=5, settleThreshold=6)
local settle_amount = 0
if (settle_threshold and settle_to) and (balance >= tonumber(settle_threshold)) and (tonumber(settle_threshold) > tonumber(settle_to)) then
    settle_amount = balance - tonumber(settle_to)

    -- Update the balance _before_ sending the settlement so that we don't accidentally send
    -- multiple settlements for the same balance. If the settlement fails we'll roll back
    -- the balance change by re-adding the amount back to the balance
    balance = settle_to
    redis.call('HSET', to_account, 'balance', balance)
end

return {balance + prepaid_amount, settle_amount}
