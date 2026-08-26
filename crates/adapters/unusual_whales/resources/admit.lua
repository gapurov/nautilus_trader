local epoch = redis.call("GET", KEYS[1])
if not epoch or epoch ~= ARGV[1] then
    return {5, 0, 0, 0, 0, 0}
end

local now_parts = redis.call("TIME")
local now_ms = tonumber(now_parts[1]) * 1000 + math.floor(tonumber(now_parts[2]) / 1000)
local lease_id = ARGV[2]
local configured_minute = tonumber(ARGV[3])
local configured_concurrency = tonumber(ARGV[4])
local daily_limit = tonumber(ARGV[5])
local lease_ttl_ms = tonumber(ARGV[6])
local window_ms = tonumber(ARGV[7])

redis.call("ZREMRANGEBYSCORE", KEYS[2], "-inf", now_ms - window_ms)
redis.call("ZREMRANGEBYSCORE", KEYS[3], "-inf", now_ms)

local observed_reset = tonumber(redis.call("HGET", KEYS[5], "observed_minute_reset_ms") or "0")
if observed_reset > 0 and observed_reset <= now_ms then
    redis.call(
        "HDEL",
        KEYS[5],
        "observed_minute_limit",
        "observed_minute_used",
        "observed_minute_reset_ms"
    )
    observed_reset = 0
end

local blocked_until = tonumber(redis.call("HGET", KEYS[5], "blocked_until_ms") or "0")
if blocked_until > now_ms then
    return {1, blocked_until - now_ms, now_ms, 0, 0, 0}
end

local effective_minute = configured_minute
local observed_minute = tonumber(redis.call("HGET", KEYS[5], "observed_minute_limit") or "0")
if observed_minute > 0 and observed_minute < effective_minute then
    effective_minute = observed_minute
end

local rolling_used = tonumber(redis.call("ZCARD", KEYS[2]))
local observed_used = tonumber(redis.call("HGET", KEYS[5], "observed_minute_used") or "0")
local minute_used = math.max(rolling_used, observed_used)
if minute_used >= effective_minute then
    local oldest = redis.call("ZRANGE", KEYS[2], 0, 0, "WITHSCORES")
    local wait_ms = window_ms
    if #oldest >= 2 then
        wait_ms = math.max(1, tonumber(oldest[2]) + window_ms - now_ms)
    elseif observed_reset > now_ms then
        wait_ms = observed_reset - now_ms
    end
    return {2, wait_ms, now_ms, effective_minute, 0, 0}
end

local effective_concurrency = configured_concurrency
local observed_concurrency = tonumber(
    redis.call("HGET", KEYS[5], "observed_concurrency_limit") or "0"
)
if observed_concurrency > 0 and observed_concurrency < effective_concurrency then
    effective_concurrency = observed_concurrency
end

local active_leases = tonumber(redis.call("ZCARD", KEYS[3]))
if active_leases >= effective_concurrency then
    local oldest_lease = redis.call("ZRANGE", KEYS[3], 0, 0, "WITHSCORES")
    local wait_ms = 1
    if #oldest_lease >= 2 then
        wait_ms = math.max(1, tonumber(oldest_lease[2]) - now_ms)
    end
    return {3, wait_ms, now_ms, effective_minute, effective_concurrency, 0}
end

local day = tostring(math.floor(now_ms / 86400000))
local daily_used = tonumber(redis.call("HGET", KEYS[4], day) or "0")
if daily_used >= daily_limit then
    local wait_ms = (math.floor(now_ms / 86400000) + 1) * 86400000 - now_ms
    return {4, wait_ms, now_ms, effective_minute, effective_concurrency, daily_used}
end

local previous_days = redis.call("HKEYS", KEYS[4])
for _, previous_day in ipairs(previous_days) do
    if previous_day ~= day then
        redis.call("HDEL", KEYS[4], previous_day)
    end
end

redis.call("ZADD", KEYS[2], now_ms, lease_id)
redis.call("ZADD", KEYS[3], now_ms + lease_ttl_ms, lease_id)
daily_used = tonumber(redis.call("HINCRBY", KEYS[4], day, 1))
return {0, 0, now_ms, effective_minute, effective_concurrency, daily_used}

