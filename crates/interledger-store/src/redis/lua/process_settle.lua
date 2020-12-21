-- almost the same as process_fulfill.lua which is used to settle when balance is over the settlement threshold:
-- returns similarly the balance and the amount to settle, but is called with only the account id as the only argument.
--
-- upon completion the `balance` is at the level of `settle_to`
local to_account = 'accounts:' .. ARGV[1]

local balance, prepaid_amount, settle_threshold, settle_to = unpack(redis.call('HMGET', to_account, 'balance', 'prepaid_amount', 'settle_threshold', 'settle_to'))
local settle_amount = 0

if (settle_threshold and settle_to) and (tonumber(settle_threshold) > tonumber(settle_to)) and tonumber(balance) >= tonumber(settle_to) then
    settle_amount = tonumber(balance) - tonumber(settle_to)
    balance = settle_to
    redis.call('HSET', to_account, 'balance', balance)
end

return {balance + prepaid_amount, settle_amount}
