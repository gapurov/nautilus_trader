local epoch = redis.call("GET", KEYS[1])
if not epoch or epoch ~= ARGV[1] then
    return -1
end

local lease_id = ARGV[2]
local lease_ttl_ms = tonumber(ARGV[3])
if not redis.call("ZSCORE", KEYS[2], lease_id) then
    return 0
end

local now_parts = redis.call("TIME")
local now_ms = tonumber(now_parts[1]) * 1000 + math.floor(tonumber(now_parts[2]) / 1000)
redis.call("ZADD", KEYS[2], "XX", now_ms + lease_ttl_ms, lease_id)
return 1
