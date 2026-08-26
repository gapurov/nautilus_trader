# Unusual Whales

Unusual Whales is an informational data provider for options flow, equities, futures, and related
market analysis.

The NautilusTrader adapter emits exact provider JSON in UW-specific custom data. It does not make
Unusual Whales an instrument or broker market-data authority.

## Capability matrix

| Capability                                 | Support                                                |
| ------------------------------------------ | ------------------------------------------------------ |
| Generated REST reads                       | All 214 GET operations in the captured OpenAPI source. |
| Account mutations                          | Cataloged, but rejected before transport.              |
| WebSocket data                             | All 28 documented join forms.                          |
| Custom data persistence                    | REST results, WebSocket events, and provider state.    |
| Native instruments                         | *Not supported.*                                       |
| Native quotes, trades, bars, books, Greeks | *Not supported.*                                       |
| Execution                                  | *Not supported.*                                       |
| Venue routing                              | *Not supported; `venue()` returns `None`.*             |
| Default routing                            | Disabled; select the client ID explicitly.             |

The captured source has 214 paths and 215 operations: 214 GET operations and one POST operation.
The POST operation is `PublicApi.AlertsController.create_config`. The adapter keeps this operation
in the generated catalog and rejects it as an account mutation.

## Requirements

Set these environment variables:

```bash
export UNUSUAL_WHALES_API_TOKEN="<token>"
export UNUSUAL_WHALES_DRAGONFLY_URL="redis://127.0.0.1:6379/"
```

Use Dragonfly for the coordination URL. The adapter checks the server identity and rejects Redis,
Valkey, and KeyDB. It has no process-local production fallback.

Dragonfly is the account-wide authority for these controls:

- Rolling-minute starts.
- Daily request use.
- Concurrent HTTP leases.
- Provider block and reset signals.
- Observed minute and concurrency limits.
- WebSocket connection starts.

All keys contain the full BLAKE3 hash of the REST base URL and API token in one hash tag. The token
is not stored in a key and is not logged.

## Node registration

Register the provider with non-default routing:

```python
from nautilus_trader.adapters.unusual_whales import UNUSUAL_WHALES
from nautilus_trader.adapters.unusual_whales import UnusualWhalesDataClientConfig
from nautilus_trader.adapters.unusual_whales import UnusualWhalesDataClientFactory
from nautilus_trader.common import Environment
from nautilus_trader.live import LiveNode
from nautilus_trader.live import RoutingConfig
from nautilus_trader.model import TraderId


node = (
    LiveNode.builder(
        "UNUSUAL-WHALES-001",
        TraderId.from_str("TRADER-001"),
        Environment.LIVE,
    )
    .add_data_client(
        UNUSUAL_WHALES,
        UnusualWhalesDataClientFactory(),
        UnusualWhalesDataClientConfig(),
        RoutingConfig(default=False),
    )
    .build()
)
```

Requests and subscriptions must use the explicit client ID
`ClientId.from_str(UNUSUAL_WHALES)`. Do not use venue routing.

## REST requests

Use `UnusualWhalesRestResult` as the custom data type name. Put the exact generated operation ID in
the data type metadata. Put path and query parameters in request parameters.

```python
from nautilus_trader.adapters.unusual_whales import UNUSUAL_WHALES
from nautilus_trader.model import DataType
from nautilus_trader.model import ClientId


data_type = DataType(
    "UnusualWhalesRestResult",
    metadata={"operation_id": "PublicApi.DarkpoolController.darkpool_ticker"},
)
self.request_data(
    data_type,
    ClientId.from_str(UNUSUAL_WHALES),
    params={"ticker": "AAPL", "limit": 25},
)
```

Local validation checks the operation, parameter names, required values, types, enums, bounds,
defaults, patterns, and array serialization before it starts a task or network request.

Each result contains:

- The operation ID and request correlation ID.
- A typed outcome.
- The HTTP status and attempt count.
- Relevant provider rate headers.
- The exact JSON response text.
- A base64 copy of the exact response bytes.
- The local receive timestamp.

Expected provider failures are result values: rate limited, entitlement denied, provider rejected,
malformed response, coordination unavailable, and transport unavailable.

## WebSocket subscriptions

Use `UnusualWhalesWebSocketEvent` as the custom data type name. Put one exact channel in the data
type metadata.

```python
data_type = DataType(
    "UnusualWhalesWebSocketEvent",
    metadata={"channel": "price:AAPL"},
)
self.subscribe_data(data_type, ClientId.from_str(UNUSUAL_WHALES))
```

The first subscription starts the connection. The adapter sends only documented join messages. A
channel becomes ready only after a positive acknowledgement on the current connection.

Each application frame produces one custom event. Valid frames keep the exact UTF-8 JSON. All
frames also keep an exact base64 byte copy, so malformed input is not dropped.

UW does not document a leave message. On the last local unsubscribe, the adapter removes local
intent and replaces the connection with the remaining desired channels.

After a reconnect, the adapter:

1. Creates a new connection ID.
1. Clears connection-bound confirmations.
1. Replays desired and pending subscriptions.
1. Emits a `ContinuityLost` provider-state event.

A join acknowledgement proves subscription state only. It does not prove that the data stream is
complete.

## Time semantics

`received_at`, `ts_event`, and `ts_init` contain the local receive time. `ts_event` exists because
the Nautilus custom-data contract requires it. It is not a provider event time. Provider timestamps
remain unchanged inside the exact JSON.

## Contract source and generation

The source snapshot is
[resources/openapi.yaml](../../crates/adapters/unusual_whales/resources/openapi.yaml). Generated
metadata records the source URL and SHA-256. The generator fails on new operation, parameter,
schema, or serialization shapes that it cannot classify.

Run:

```bash
ruby crates/adapters/unusual_whales/scripts/generate_contract.rb --fetch
ruby crates/adapters/unusual_whales/scripts/generate_contract.rb \
  --source crates/adapters/unusual_whales/resources/openapi.yaml \
  --check
```

The build and runtime do not access the OpenAPI network source.

The provider assigns opaque `DynLimit...` component names when it exports the document.
Two downloads can have different byte hashes while operation, parameter, schema, and channel
semantics stay equal. Treat the committed source SHA-256 as snapshot identity and review normalized
semantic output when a later fetch changes only these aliases.

## Tests

Normal tests use captured source and malformed variants. Dragonfly integration tests run only when
`UNUSUAL_WHALES_DRAGONFLY_TEST_URL` points to a dedicated Dragonfly test service.

The controlled-account smoke test is ignored by default:

```bash
cargo test -p nautilus-unusual-whales \
  --test live_smoke \
  -- --ignored
```

This test needs `UNUSUAL_WHALES_API_TOKEN` and `UNUSUAL_WHALES_DRAGONFLY_URL`.

## References

- [Unusual Whales API documentation](https://api.unusualwhales.com/docs)
- [Official OpenAPI source](https://api.unusualwhales.com/api/openapi)
- [Dragonfly Lua scripting](https://www.dragonflydb.io/blog/leveraging-power-of-lua-scripting)
- [NautilusTrader adapter guide](../developer_guide/adapters.md)
