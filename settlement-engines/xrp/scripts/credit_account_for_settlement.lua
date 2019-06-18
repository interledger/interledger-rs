local xrp_address = ARGV[1]
local drops = ARGV[2]

if not (xrp_address and drops) then
    error('xrp_address and drops are required')
end

local account = redis.call('HGET', 'xrp_addresses', ARGV[1])
if account then
    local asset_scale = tonumber(redis.call('HGET', 'accounts:' .. account, 'asset_scale'))
    local scaled_amount = math.floor(drops * 10 ^ (asset_scale - 6))
    local new_balance = redis.call('HINCRBY', 'balances:xrp', '' .. account, scaled_amount)
    return {account, new_balance}
else
    error('No account associated with address: ' .. ARGV[1])
end
