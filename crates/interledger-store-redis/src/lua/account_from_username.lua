local username = ARGV[1]
if redis.call('HEXISTS', 'usernames', username) then
    local id = redis.call('HGET', 'usernames', ARGV[1])
    return redis.call('HGETALL', 'accounts:' .. id)
else
    return nil
end
