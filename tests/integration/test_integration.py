"""Integration tests for Sleeper MCP server.

These tests verify the complete integration between our MCP server,
the Sleeper API, and Goose interaction patterns.
"""

import json
import os
from typing import Any, AsyncGenerator, Dict, Generator
import pytest
import pytest_asyncio
from fastapi import FastAPI
from httpx import AsyncClient
import respx
from structlog.testing import capture_logs

from src.config import Config, get_config
from src.main import app
from src.mcp import MCPResponseStatus
from src.services import SleeperAPIClient

# Test data fixtures
MOCK_USER_DATA = {
    "username": "testuser",
    "user_id": "123456789",
    "display_name": "Test User",
    "avatar": "abc123"
}

MOCK_LEAGUE_DATA = {
    "league_id": "123456789",
    "name": "Test League",
    "season": "2023",
    "status": "in_season",
    "sport": "nfl",
    "settings": {
        "draft_type": "snake",
        "num_teams": 12,
        "scoring_type": "ppr"
    },
    "scoring_settings": {
        "reception": 1.0,
        "passing_td": 4
    },
    "roster_positions": ["QB", "RB", "RB", "WR", "WR", "TE", "FLEX", "K", "DEF"],
    "total_rosters": 12
}

MOCK_ROSTER_DATA = [
    {
        "roster_id": 1,
        "owner_id": "123456789",
        "league_id": "123456789",
        "players": ["1234", "5678"],
        "starters": ["1234"],
        "settings": {
            "wins": 5,
            "losses": 3,
            "ties": 0,
            "fpts": 750.5
        }
    }
]

MOCK_NFL_STATE = {
    "week": 10,
    "season_type": "regular",
    "season": "2023",
    "previous_season": "2022",
    "league_season": "2023",
    "league_create_season": "2023",
    "display_week": 10
}


@pytest.fixture(scope="session")
def mock_sleeper_api():
    """Create a mock Sleeper API using respx."""
    with respx.mock(base_url="https://api.sleeper.app/v1", assert_all_called=False) as respx_mock:
        # Mock user endpoints
        respx_mock.get("/user/testuser").mock(return_value=MOCK_USER_DATA)
        respx_mock.get("/user/123456789").mock(return_value=MOCK_USER_DATA)
        
        # Mock league endpoints
        respx_mock.get("/user/123456789/leagues/nfl/2023").mock(
            return_value=[MOCK_LEAGUE_DATA]
        )
        respx_mock.get("/league/123456789").mock(return_value=MOCK_LEAGUE_DATA)
        respx_mock.get("/league/123456789/rosters").mock(return_value=MOCK_ROSTER_DATA)
        respx_mock.get("/league/123456789/users").mock(return_value=[MOCK_USER_DATA])
        
        # Mock NFL state endpoint
        respx_mock.get("/state/nfl").mock(return_value=MOCK_NFL_STATE)
        
        yield respx_mock


@pytest_asyncio.fixture
async def test_client(mock_sleeper_api) -> AsyncGenerator[AsyncClient, None]:
    """Create an async test client for the FastAPI application."""
    async with AsyncClient(app=app, base_url="http://test") as client:
        yield client


@pytest.mark.asyncio
async def test_health_check_integration(test_client: AsyncClient):
    """Test health check endpoint with mock API."""
    with capture_logs() as cap_logs:
        response = await test_client.get("/health")
        
    assert response.status_code == 200
    data = response.json()
    assert data["status"] == "healthy"
    
    # Verify logging
    assert any(
        log["event"] == "application_startup"
        for log in cap_logs
    )


@pytest.mark.asyncio
async def test_capabilities_integration(test_client: AsyncClient):
    """Test capabilities endpoint returns enhanced functions."""
    response = await test_client.get("/capabilities")
    assert response.status_code == 200
    
    data = response.json()
    assert "functions" in data
    assert "version" in data
    assert "provider" in data
    assert data["provider"] == "sleeper"
    
    # Verify enhanced function descriptions
    functions = {func["name"]: func for func in data["functions"]}
    assert "get_league" in functions
    league_func = functions["get_league"]
    assert "strategy" in league_func["description"].lower()


@pytest.mark.asyncio
@pytest.mark.parametrize("function_name,parameters,expected_status", [
    (
        "get_user",
        {"identifier": "testuser"},
        MCPResponseStatus.SUCCESS
    ),
    (
        "get_league",
        {"league_id": "123456789"},
        MCPResponseStatus.SUCCESS
    ),
    (
        "get_user",
        {"identifier": "nonexistent"},
        MCPResponseStatus.ERROR
    ),
])
async def test_function_invocation_integration(
    test_client: AsyncClient,
    function_name: str,
    parameters: Dict[str, Any],
    expected_status: MCPResponseStatus
):
    """Test function invocation with different scenarios."""
    with capture_logs() as cap_logs:
        response = await test_client.post(
            "/invoke",
            json={
                "function_name": function_name,
                "parameters": parameters
            }
        )
    
    assert response.status_code == 200
    data = response.json()
    assert data["status"] == expected_status
    
    if expected_status == MCPResponseStatus.SUCCESS:
        assert data["result"] is not None
        assert data["error"] is None
        
        # Verify context enhancement
        if function_name == "get_league":
            assert "context" in data["result"]
            assert "suggested_strategies" in data["result"]["context"]
    else:
        assert data["error"] is not None
        
    # Verify proper logging
    assert any(
        log["event"] in ("executing_function", "function_execution_failed")
        for log in cap_logs
    )


@pytest.mark.asyncio
async def test_enhanced_league_response(test_client: AsyncClient):
    """Test that league responses include enhanced fantasy context."""
    response = await test_client.post(
        "/invoke",
        json={
            "function_name": "get_league",
            "parameters": {"league_id": "123456789"}
        }
    )
    
    assert response.status_code == 200
    data = response.json()
    assert data["status"] == MCPResponseStatus.SUCCESS
    
    result = data["result"]
    assert "context" in result
    
    context = result["context"]
    assert "league_type_info" in context
    assert "suggested_strategies" in context
    
    # Verify PPR detection
    league_type = context["league_type_info"]
    assert "ppr" in league_type["name"].lower()
    
    # Verify strategy suggestions
    strategies = context["suggested_strategies"]
    assert any("receiver" in strategy.lower() for strategy in strategies)


@pytest.mark.asyncio
async def test_enhanced_roster_response(test_client: AsyncClient):
    """Test that roster responses include position information."""
    response = await test_client.post(
        "/invoke",
        json={
            "function_name": "get_league_rosters",
            "parameters": {"league_id": "123456789"}
        }
    )
    
    assert response.status_code == 200
    data = response.json()
    assert data["status"] == MCPResponseStatus.SUCCESS
    
    result = data["result"]
    assert isinstance(result, list)
    assert len(result) > 0
    
    # Verify position information is included
    first_roster = result[0]
    assert "context" in first_roster
    position_info = first_roster["context"].get("position_info", {})
    assert len(position_info) > 0
    
    # Verify position details
    for pos, info in position_info.items():
        assert "name" in info
        assert "scoring_relevance" in info
        assert "typical_roster_spots" in info


@pytest.mark.asyncio
async def test_rate_limiting_integration(test_client: AsyncClient):
    """Test rate limiting behavior with rapid requests."""
    config = get_config()
    requests_to_send = config.sleeper_api.rate_limit_per_minute + 1
    
    responses = []
    for _ in range(requests_to_send):
        response = await test_client.post(
            "/invoke",
            json={
                "function_name": "get_nfl_state",
                "parameters": {}
            }
        )
        responses.append(response)
    
    # Verify rate limiting
    success_responses = [r for r in responses if r.json()["status"] == MCPResponseStatus.SUCCESS]
    error_responses = [r for r in responses if r.json()["status"] == MCPResponseStatus.ERROR]
    
    assert len(success_responses) <= config.sleeper_api.rate_limit_per_minute
    assert len(error_responses) > 0
    
    # Verify rate limit error message
    error_response = error_responses[0].json()
    assert "rate limit" in error_response["error"].lower()


@pytest.mark.asyncio
async def test_caching_integration(test_client: AsyncClient, mock_sleeper_api):
    """Test that responses are properly cached."""
    # Make initial request
    response1 = await test_client.post(
        "/invoke",
        json={
            "function_name": "get_user",
            "parameters": {"identifier": "testuser"}
        }
    )
    
    # Make same request again
    response2 = await test_client.post(
        "/invoke",
        json={
            "function_name": "get_user",
            "parameters": {"identifier": "testuser"}
        }
    )
    
    # Both should succeed
    assert response1.status_code == 200
    assert response2.status_code == 200
    
    # Verify only one actual API call was made
    user_route = mock_sleeper_api.routes[0]  # /user/testuser route
    assert user_route.call_count == 1


@pytest.mark.asyncio
async def test_error_handling_integration(test_client: AsyncClient):
    """Test error handling with various error scenarios."""
    test_cases = [
        {
            "function_name": "nonexistent_function",
            "parameters": {},
            "expected_error": "Unknown function"
        },
        {
            "function_name": "get_user",
            "parameters": {},  # Missing required parameter
            "expected_error": "identifier"
        },
        {
            "function_name": "get_league",
            "parameters": {"league_id": "invalid"},
            "expected_error": "league"
        }
    ]
    
    for test_case in test_cases:
        response = await test_client.post(
            "/invoke",
            json=test_case
        )
        
        assert response.status_code == 200  # MCP always returns 200
        data = response.json()
        assert data["status"] == MCPResponseStatus.ERROR
        assert test_case["expected_error"].lower() in data["error"].lower()


@pytest.mark.asyncio
async def test_logging_integration(test_client: AsyncClient):
    """Test that important operations are properly logged."""
    with capture_logs() as cap_logs:
        # Perform various operations
        await test_client.get("/health")
        await test_client.get("/capabilities")
        await test_client.post(
            "/invoke",
            json={
                "function_name": "get_user",
                "parameters": {"identifier": "testuser"}
            }
        )
        
        # Force an error
        await test_client.post(
            "/invoke",
            json={
                "function_name": "invalid_function",
                "parameters": {}
            }
        )
    
    # Verify log entries
    log_events = [log["event"] for log in cap_logs]
    
    assert "application_startup" in log_events
    assert "executing_function" in log_events
    assert any(
        "error" in event.lower()
        for event in log_events
    )
    
    # Verify log contents
    function_logs = [
        log for log in cap_logs
        if log["event"] == "executing_function"
    ]
    assert len(function_logs) > 0
    assert "function" in function_logs[0]
    assert "parameters" in function_logs[0]
