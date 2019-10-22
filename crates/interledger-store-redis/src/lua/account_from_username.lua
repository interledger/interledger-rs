local username = ARGV[1]
local id_from_username = redis.call('HGET', 'usernames', username)
if id_from_username then
    return redis.call('HGETALL', 'accounts:' .. id_from_username)
else
    return nil
end
