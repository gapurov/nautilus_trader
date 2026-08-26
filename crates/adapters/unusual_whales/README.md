# Unusual Whales

This crate provides the native Unusual Whales informational data adapter for NautilusTrader.

The adapter supports generated REST read operations and documented WebSocket channels as custom
data. It does not provide instruments, native market data, execution, a venue, or default routing.

See the [integration guide](../../../docs/integrations/unusual_whales.md).

## Contract generation

The repository contains the official OpenAPI source snapshot and deterministic generated output.
Regenerate from the network only during development:

```bash
ruby crates/adapters/unusual_whales/scripts/generate_contract.rb --fetch
```

Verify committed output without network access:

```bash
ruby crates/adapters/unusual_whales/scripts/generate_contract.rb \
  --source crates/adapters/unusual_whales/resources/openapi.yaml \
  --check
```
