local usernames_key = ARGV[1]
local accounts_key = ARGV[2]
local username = ARGV[3]
local id_from_username = redis.call('HGET', usernames_key, username)
if id_from_username then
    return redis.call('HGETALL', accounts_key .. ':' .. id_from_username)
else
    return nil
end
