local epoch = redis.call("GET", KEYS[1])
if not epoch or epoch ~= ARGV[1] then
    return -1
end

return redis.call("ZREM", KEYS[2], ARGV[2])
