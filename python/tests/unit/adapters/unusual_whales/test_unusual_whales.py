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

import ast
from pathlib import Path

import nautilus_trader.adapters.unusual_whales as unusual_whales
from nautilus_trader.adapters.unusual_whales import CHANNEL_FORM_COUNT
from nautilus_trader.adapters.unusual_whales import OPERATION_COUNT
from nautilus_trader.adapters.unusual_whales import UnusualWhalesDataClientConfig
from nautilus_trader.adapters.unusual_whales import UnusualWhalesDataClientFactory
from nautilus_trader.adapters.unusual_whales import UnusualWhalesWebSocketEvent
from nautilus_trader.adapters.unusual_whales import unusual_whales_channel_forms
from nautilus_trader.adapters.unusual_whales import unusual_whales_operation_ids
from nautilus_trader.common import Environment
from nautilus_trader.live import LiveNode
from nautilus_trader.live import RoutingConfig
from nautilus_trader.model import TraderId


def test_import_surface_and_generated_contract_counts() -> None:
    assert OPERATION_COUNT == 215
    assert CHANNEL_FORM_COUNT == 28
    assert len(unusual_whales_operation_ids()) == 215
    assert len(unusual_whales_channel_forms()) == 28
    assert "PublicApi.AlertsController.create_config" in unusual_whales_operation_ids()
    assert "futures:TICKER" in unusual_whales_channel_forms()
    assert set(unusual_whales.__all__) <= set(dir(unusual_whales))


def test_config_and_factory_redact_secrets() -> None:
    config = UnusualWhalesDataClientConfig(
        api_key="secret-token",
        dragonfly_url="redis://user:secret@127.0.0.1:6379/",
    )
    assert config.has_api_key is True
    assert config.has_dragonfly_url is True
    assert repr(config) == "UnusualWhalesDataClientConfig"
    assert UnusualWhalesDataClientFactory().name() == "UNUSUAL_WHALES"


def test_builder_registers_explicit_non_default_data_client() -> None:
    builder = LiveNode.builder(
        "UW-BUILDER-TEST",
        TraderId.from_str("TESTER-001"),
        Environment.LIVE,
    )
    result = builder.add_data_client(
        "UNUSUAL_WHALES",
        UnusualWhalesDataClientFactory(),
        UnusualWhalesDataClientConfig(
            api_key="test-token",
            dragonfly_url="redis://127.0.0.1:6379/",
        ),
        RoutingConfig(default=False),
    )
    assert result is not None


def test_custom_data_python_conversion_preserves_exact_frame() -> None:
    frame = '{"channel":"price:AAPL","provider_ts":123}'
    event = UnusualWhalesWebSocketEvent(
        channel="price:AAPL",
        connection_id="connection-1",
        frame_json=frame,
        frame_body_base64="eyJjaGFubmVsIjoicHJpY2U6QUFQTCIsInByb3ZpZGVyX3RzIjoxMjN9",
        is_valid_json=True,
        received_at=10,
        ts_event=10,
        ts_init=10,
    )
    assert event.frame_json == frame
    assert event.channel == "price:AAPL"
    assert event.is_valid_json is True


def test_stub_exports_match_runtime_facade() -> None:
    stub_path = Path(unusual_whales.__file__).with_name("__init__.pyi")
    module = ast.parse(stub_path.read_text())
    assignment = next(
        node
        for node in module.body
        if isinstance(node, ast.Assign)
        and any(isinstance(target, ast.Name) and target.id == "__all__" for target in node.targets)
    )
    assert isinstance(assignment.value, ast.List)
    stub_exports = [element.value for element in assignment.value.elts]
    assert stub_exports == unusual_whales.__all__
