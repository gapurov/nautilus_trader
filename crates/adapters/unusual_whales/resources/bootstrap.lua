local current = redis.call("GET", KEYS[1])
if current then
    return {current, 0}
end

redis.call("SET", KEYS[1], ARGV[1])
return {ARGV[1], 1}

