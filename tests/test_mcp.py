"""Test the MCP protocol implementation."""

from typing import Any, Dict
from unittest.mock import Mock, patch

import pytest
from fastapi import HTTPException
from httpx import AsyncClient

from src.mcp import (
    MCPHandler,
    MCPInvocation,
    MCPResponse,
    MCPResponseStatus,
    get_mcp_capabilities,
)
from src.models import NFLState, User
from src.services import SleeperAPIClient


@pytest.fixture
def mock_sleeper_client():
    """Create a mock Sleeper API client."""
    return Mock(spec=SleeperAPIClient)


@pytest.fixture
def mcp_handler(mock_sleeper_client):
    """Create an MCP handler with a mock client."""
    return MCPHandler(mock_sleeper_client)


@pytest.mark.asyncio
async def test_capabilities(async_client: AsyncClient):
    """Test the capabilities endpoint."""
    response = await async_client.get("/capabilities")
    assert response.status_code == 200
    
    data = response.json()
    assert "functions" in data
    assert "version" in data
    assert "provider" in data
    assert data["provider"] == "sleeper"
    
    # Verify function definitions
    functions = data["functions"]
    assert isinstance(functions, list)
    assert len(functions) > 0
    
    # Check a specific function
    get_user = next(f for f in functions if f["name"] == "get_user")
    assert "parameters" in get_user
    assert len(get_user["parameters"]) == 1
    assert get_user["parameters"][0]["name"] == "identifier"


@pytest.mark.asyncio
async def test_function_invocation_success(
    async_client: AsyncClient,
    mock_sleeper_client,
):
    """Test successful function invocation."""
    test_user = {
        "username": "testuser",
        "user_id": "123",
        "display_name": "Test User",
        "avatar": None,
    }
    
    # Setup mock
    mock_sleeper_client.get_user.return_value = User(**test_user)
    
    # Make request
    response = await async_client.post(
        "/invoke",
        json={
            "function_name": "get_user",
            "parameters": {"identifier": "testuser"},
        },
    )
    
    assert response.status_code == 200
    data = response.json()
    assert data["status"] == MCPResponseStatus.SUCCESS
    assert data["result"] == test_user
    assert data["error"] is None


@pytest.mark.asyncio
async def test_function_invocation_error(
    async_client: AsyncClient,
    mock_sleeper_client,
):
    """Test error handling in function invocation."""
    # Setup mock to raise an error
    mock_sleeper_client.get_user.side_effect = HTTPException(
        status_code=404,
        detail="User not found",
    )
    
    # Make request
    response = await async_client.post(
        "/invoke",
        json={
            "function_name": "get_user",
            "parameters": {"identifier": "nonexistent"},
        },
    )
    
    assert response.status_code == 200  # MCP always returns 200
    data = response.json()
    assert data["status"] == MCPResponseStatus.ERROR
    assert "User not found" in data["error"]
    assert data["result"] is None


@pytest.mark.asyncio
async def test_invalid_function(async_client: AsyncClient):
    """Test invoking a non-existent function."""
    response = await async_client.post(
        "/invoke",
        json={
            "function_name": "nonexistent_function",
            "parameters": {},
        },
    )
    
    assert response.status_code == 200
    data = response.json()
    assert data["status"] == MCPResponseStatus.ERROR
    assert "Unknown function" in data["error"]


@pytest.mark.asyncio
async def test_mcp_handler_result_serialization(mcp_handler: MCPHandler):
    """Test that the MCP handler properly serializes different result types."""
    # Test with Pydantic model
    test_user = User(
        username="test",
        user_id="123",
        display_name="Test User",
        avatar=None,
    )
    mcp_handler.client.get_user.return_value = test_user
    
    result = await mcp_handler.execute_function(
        MCPInvocation(
            function_name="get_user",
            parameters={"identifier": "test"},
        )
    )
    
    assert result.status == MCPResponseStatus.SUCCESS
    assert isinstance(result.result, dict)
    assert result.result["username"] == "test"
    
    # Test with list of models
    mcp_handler.client.get_user_leagues.return_value = []
    result = await mcp_handler.execute_function(
        MCPInvocation(
            function_name="get_user_leagues",
            parameters={"user_id": "123", "season": "2023"},
        )
    )
    
    assert result.status == MCPResponseStatus.SUCCESS
    assert isinstance(result.result, list)
