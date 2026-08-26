#!/usr/bin/env python3
# -------------------------------------------------------------------------------------------------
#  Copyright (C) 2015-2026 Nautech Systems Pty Ltd. All rights reserved.
#  https://nautechsystems.io
#
#  Licensed under the GNU Lesser General Public License Version 3.0 (the "License");
#  You may not use this file except in compliance with the License.
#  You may obtain a copy of the License at https://www.gnu.org/licenses/lgpl-3.0.en.html
#
#  Unless required by applicable law or agreed to in writing, software
#  distributed under the License is distributed on an "AS IS" BASIS,
#  WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
#  See the License for the specific language governing permissions and
#  limitations under the License.
# -------------------------------------------------------------------------------------------------

from nautilus_trader.adapters.unusual_whales import UNUSUAL_WHALES
from nautilus_trader.adapters.unusual_whales import UnusualWhalesDataClientConfig
from nautilus_trader.adapters.unusual_whales import UnusualWhalesDataClientFactory
from nautilus_trader.common import Environment
from nautilus_trader.config import StrategyConfig
from nautilus_trader.core import Data
from nautilus_trader.live import LiveNode
from nautilus_trader.live import RoutingConfig
from nautilus_trader.model import DataType
from nautilus_trader.model import ClientId
from nautilus_trader.model import TraderId
from nautilus_trader.trading import Strategy


# Set UNUSUAL_WHALES_API_TOKEN and UNUSUAL_WHALES_DRAGONFLY_URL before use.
TRADER_ID = TraderId.from_str("TESTER-001")
REST_OPERATION = "PublicApi.MarketController.market_tide"
WEBSOCKET_CHANNEL = "price:AAPL"
CLIENT_ID = ClientId.from_str(UNUSUAL_WHALES)


class UnusualWhalesTesterConfig(StrategyConfig, frozen=True):
    rest_operation: str
    websocket_channel: str


class UnusualWhalesTester(Strategy):
    def on_start(self) -> None:
        rest_type = DataType(
            "UnusualWhalesRestResult",
            metadata={"operation_id": self.config.rest_operation},
        )
        stream_type = DataType(
            "UnusualWhalesWebSocketEvent",
            metadata={"channel": self.config.websocket_channel},
        )
        self.request_data(rest_type, CLIENT_ID)
        self.subscribe_data(stream_type, CLIENT_ID)

    def on_data(self, data: Data) -> None:
        self.log.info(repr(data))

    def on_historical_data(self, data: Data) -> None:
        self.log.info(repr(data))


def main() -> None:
    node = (
        LiveNode.builder("UNUSUAL-WHALES-DATA-TESTER-001", TRADER_ID, Environment.LIVE)
        .add_data_client(
            UNUSUAL_WHALES,
            UnusualWhalesDataClientFactory(),
            UnusualWhalesDataClientConfig(),
            RoutingConfig(default=False),
        )
        .build()
    )
    node.add_strategy(
        UnusualWhalesTester(
            UnusualWhalesTesterConfig(
                rest_operation=REST_OPERATION,
                websocket_channel=WEBSOCKET_CHANNEL,
            )
        )
    )
    try:
        node.run()
    finally:
        node.dispose()


if __name__ == "__main__":
    main()
