local epoch = redis.call("GET", KEYS[1])
if not epoch or epoch ~= ARGV[1] then
    return {2, 0, 0}
end

local now_parts = redis.call("TIME")
local now_ms = tonumber(now_parts[1]) * 1000 + math.floor(tonumber(now_parts[2]) / 1000)
local interval_ms = tonumber(ARGV[2])
local blocked_until = tonumber(redis.call("HGET", KEYS[2], "reconnect_until_ms") or "0")
if blocked_until > now_ms then
    return {1, blocked_until - now_ms, now_ms}
end

redis.call("HSET", KEYS[2], "reconnect_until_ms", now_ms + interval_ms)
return {0, 0, now_ms}

