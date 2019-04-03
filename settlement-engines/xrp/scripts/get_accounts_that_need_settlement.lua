-- The cursor is used to page through the results.
-- This script should be called first with '0'.
-- If the cursor returned is '0', that means it has finished going through all of the accounts.
-- If the cursor returned is not '0', this script should be called again with
-- that returned value to iterate through the next set of accounts.
local cursor = ARGV[1]

-- Once this number of accounts is reached, the script will stop paging through the balances.
-- The number of accounts returned MAY be greater than this number, as the script will finish checking the
-- page of results it is scanning through before returning.
local stop_after_num_accounts = tonumber(ARGV[2]) or 20

local min_drops_to_settle = tonumber(ARGV[3]) or 0

local accounts = {}
local balances = nil

-- Scan the balances until we've either filled up the accounts to return or we've hit the end
-- TODO should this only go through one HSCAN page to make sure we don't block the db for too long?
repeat
    cursor, balances = unpack(redis.call("HSCAN", "balances:xrp", 0))
    for i = 1, table.getn(balances), 2 do
        local account = balances[i]
        local balance = tonumber(balances[i + 1])

        local settle_threshold = redis.call("HGET", "accounts:" .. account, "settle_threshold")
        -- Note: this ignores accounts that are missing any of the required details
        if settle_threshold then
            -- Check whether this account needs to settle
            if balance >= tonumber(settle_threshold) then
                local xrp_address, asset_scale, settle_to =
                    unpack(redis.call("HMGET", "accounts:" .. account, "xrp_address", "asset_scale", "settle_to"))

                if (xrp_address and asset_scale and settle_to) then
                    -- Calculate the amount in XRP drops that the account needs to settle
                    local amount_to_settle = balance - tonumber(settle_to)
                    local drops_to_settle = math.floor(amount_to_settle * 10 ^ (6 - asset_scale))

                    if drops_to_settle >= min_drops_to_settle then
                        table.insert(accounts, {account, xrp_address, drops_to_settle})
                    end
                end
            end
        end
    end
until table.getn(accounts) >= stop_after_num_accounts or cursor == "0"

return {cursor, accounts}
