local account = ARGV[1]
local drops = ARGV[2]

if not (account and drops) then
    error('account and drops are required')
end

local asset_scale = redis.call('HGET', 'accounts:' .. account, 'asset_scale')
if not asset_scale then
    error('account ' .. account .. ' is missing asset_scale')
end

-- XRP drops have a scale of 6
local amount = math.floor(drops * 10 ^ (asset_scale - 6))
-- The balance represents how much we owe them so lower it to reflect that we settled
local new_balance = redis.call('HINCRBY', 'balances:xrp', account, 0 - amount)

return new_balance