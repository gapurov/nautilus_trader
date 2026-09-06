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
"""
Request REST data and stream Unusual Whales custom data.
"""

from nautilus_trader.adapters.unusual_whales import UNUSUAL_WHALES
from nautilus_trader.adapters.unusual_whales import UnusualWhalesDataClientConfig
from nautilus_trader.adapters.unusual_whales import UnusualWhalesDataClientFactory
from nautilus_trader.common import Environment
from nautilus_trader.live import LiveNode
from nautilus_trader.live import RoutingConfig
from nautilus_trader.model import ClientId
from nautilus_trader.model import DataType
from nautilus_trader.model import TraderId
from nautilus_trader.trading import Strategy


# Set UNUSUAL_WHALES_API_TOKEN and UNUSUAL_WHALES_DRAGONFLY_URL before use.
TRADER_ID = TraderId.from_str("TESTER-001")
REST_OPERATION = "PublicApi.MarketController.market_tide"
WEBSOCKET_CHANNEL = "price:AAPL"
CLIENT_ID = ClientId.from_str(UNUSUAL_WHALES)


class UnusualWhalesTester(Strategy):
    """
    Log the requested REST response and subscribed WebSocket events.
    """

    def __init__(self, rest_operation: str, websocket_channel: str) -> None:
        """
        Set the REST operation and WebSocket channel.
        """
        super().__init__()
        self._rest_operation = rest_operation
        self._websocket_channel = websocket_channel

    def on_start(self) -> None:
        """
        Request REST data and subscribe to the configured channel.
        """
        rest_type = DataType(
            "UnusualWhalesRestResult",
            metadata={"operation_id": self._rest_operation},
        )
        stream_type = DataType(
            "UnusualWhalesWebSocketEvent",
            metadata={"channel": self._websocket_channel},
        )
        self.request_data(rest_type, CLIENT_ID)
        self.subscribe_data(stream_type, CLIENT_ID)

    def on_data(self, data: object) -> None:
        """
        Log a custom data event.
        """
        self.log.info(repr(data))

    def on_historical_data(self, data: object) -> None:
        """
        Log a historical custom data response.
        """
        self.log.info(repr(data))


def main() -> None:
    """
    Run the Unusual Whales data example.
    """
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
            rest_operation=REST_OPERATION,
            websocket_channel=WEBSOCKET_CHANNEL,
        ),
    )
    try:
        node.run()
    finally:
        node.dispose()


if __name__ == "__main__":
    main()
