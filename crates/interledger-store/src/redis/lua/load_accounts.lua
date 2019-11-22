-- borrowed from https://stackoverflow.com/a/34313599
local function into_dictionary(flat_map)
    local result = {}
    for i = 1, #flat_map, 2 do
        result[flat_map[i]] = flat_map[i + 1]
    end
    return result
end

local settlement_engines = into_dictionary(redis.call('HGETALL', 'settlement_engines'))
local accounts = {}

-- TODO get rid of the two representations of account
-- For some reason, the result from HGETALL returns
-- a bulk value type that we can return but that
-- we cannot index into with string keys. In contrast,
-- the result from into_dictionary can be indexed into
-- but if we try to return it, redis thinks it is a
-- '(empty list or set)'. There _should_ be some better way to do
-- this simple operation and a less janky way to insert the
-- settlement_engine_url into the account we are going to return
local account
local account_dict
for index, id in ipairs(ARGV) do
    account = redis.call('HGETALL', 'accounts:' .. id)

    if account ~= nil then
        account_dict = into_dictionary(account)

        -- If the account does not have a settlement_engine_url specified
        -- but there is one configured for that currency, set the
        -- account to use that url
        if account_dict.settlement_engine_url == nil then
            local url = settlement_engines[account_dict.asset_code]
            if url ~= nil then
                table.insert(account, 'settlement_engine_url')
                table.insert(account, url)
            end
        end

        table.insert(accounts, account)
    end
end
return accounts
