local epoch = redis.call("GET", KEYS[1])
if not epoch or epoch ~= ARGV[1] then
    return {0, 0}
end

local now_parts = redis.call("TIME")
local now_ms = tonumber(now_parts[1]) * 1000 + math.floor(tonumber(now_parts[2]) / 1000)
local retry_after_until_ms = tonumber(ARGV[2])
local observed_counter = tonumber(ARGV[3])
local observed_remaining = tonumber(ARGV[4])
local observed_reset_ms = tonumber(ARGV[5])
local success = tonumber(ARGV[6])
local concurrency_exceeded = tonumber(ARGV[7])
local configured_concurrency = tonumber(ARGV[8])

redis.call("ZREMRANGEBYSCORE", KEYS[2], "-inf", now_ms)

local current_reset = tonumber(redis.call("HGET", KEYS[3], "observed_minute_reset_ms") or "0")
if current_reset > 0 and current_reset <= now_ms then
    redis.call(
        "HDEL",
        KEYS[3],
        "observed_minute_limit",
        "observed_minute_used",
        "observed_minute_reset_ms"
    )
    current_reset = 0
end

if observed_reset_ms > now_ms then
    if current_reset == 0 then
        current_reset = observed_reset_ms
        redis.call("HSET", KEYS[3], "observed_minute_reset_ms", current_reset)
    elseif observed_reset_ms > current_reset then
        current_reset = observed_reset_ms
        redis.call("HSET", KEYS[3], "observed_minute_reset_ms", current_reset)
    end

    if observed_counter >= 0 then
        local current_used = tonumber(redis.call("HGET", KEYS[3], "observed_minute_used") or "0")
        if observed_counter > current_used then
            redis.call("HSET", KEYS[3], "observed_minute_used", observed_counter)
        end
    end

    if observed_counter >= 0 and observed_remaining >= 0 then
        local candidate_limit = observed_counter + observed_remaining
        local current_limit = tonumber(
            redis.call("HGET", KEYS[3], "observed_minute_limit") or "0"
        )
        if candidate_limit > 0 and (current_limit == 0 or candidate_limit < current_limit) then
            redis.call("HSET", KEYS[3], "observed_minute_limit", candidate_limit)
        end
    end
end

if concurrency_exceeded == 1 then
    local active_leases = tonumber(redis.call("ZCARD", KEYS[2]))
    local candidate = math.max(1, math.min(configured_concurrency, active_leases - 1))
    local current = tonumber(redis.call("HGET", KEYS[3], "observed_concurrency_limit") or "0")
    if current == 0 or candidate < current then
        redis.call("HSET", KEYS[3], "observed_concurrency_limit", candidate)
    end
else
    local block_until = retry_after_until_ms
    if success == 1 and observed_remaining == 0 and observed_reset_ms > block_until then
        block_until = observed_reset_ms
    end
    if block_until > now_ms then
        local current_block = tonumber(redis.call("HGET", KEYS[3], "blocked_until_ms") or "0")
        if block_until > current_block then
            redis.call("HSET", KEYS[3], "blocked_until_ms", block_until)
        end
    end
end

return {1, now_ms}

