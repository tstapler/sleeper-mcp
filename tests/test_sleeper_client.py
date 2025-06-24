"""Test the Sleeper API client."""

from datetime import datetime
from typing import Any, Dict
from unittest.mock import Mock, patch

import pytest
from fastapi import HTTPException
import httpx

from src.config import Config
from src.models import NFLState, User
from src.services.sleeper import SleeperAPIClient


@pytest.fixture
def mock_config():
    """Create a mock configuration."""
    return Config()


@pytest.fixture
def mock_response():
    """Create a mock HTTP response."""
    def _create_response(data: Dict[str, Any], status_code: int = 200) -> Mock:
        response = Mock(spec=httpx.Response)
        response.status_code = status_code
        response.json.return_value = data
        return response
    return _create_response


@pytest.mark.asyncio
async def test_get_user(mock_config: Config, mock_response):
    """Test getting a user from the API."""
    test_user_data = {
        "username": "testuser",
        "user_id": "123456",
        "display_name": "Test User",
        "avatar": "abc123",
    }
    
    with patch("httpx.AsyncClient.request") as mock_request:
        mock_request.return_value = mock_response(test_user_data)
        
        client = SleeperAPIClient(mock_config)
        user = await client.get_user("testuser")
        
        assert isinstance(user, User)
        assert user.username == test_user_data["username"]
        assert user.user_id == test_user_data["user_id"]
        assert user.display_name == test_user_data["display_name"]
        assert user.avatar == test_user_data["avatar"]


@pytest.mark.asyncio
async def test_rate_limiting(mock_config: Config, mock_response):
    """Test that rate limiting is enforced."""
    mock_config.sleeper_api.rate_limit_per_minute = 2
    test_data = {"week": 1, "season": "2023"}
    
    with patch("httpx.AsyncClient.request") as mock_request:
        mock_request.return_value = mock_response(test_data)
        
        client = SleeperAPIClient(mock_config)
        
        # First two requests should succeed
        await client.get_nfl_state()
        await client.get_nfl_state()
        
        # Third request should fail
        with pytest.raises(HTTPException) as exc_info:
            await client.get_nfl_state()
        
        assert exc_info.value.status_code == 429


@pytest.mark.asyncio
async def test_caching(mock_config: Config, mock_response):
    """Test that responses are cached."""
    test_data = {"week": 1, "season": "2023"}
    
    with patch("httpx.AsyncClient.request") as mock_request:
        mock_request.return_value = mock_response(test_data)
        
        client = SleeperAPIClient(mock_config)
        
        # First request should hit the API
        result1 = await client.get_nfl_state()
        
        # Second request should use cache
        result2 = await client.get_nfl_state()
        
        assert mock_request.call_count == 1  # Only one actual API call
        assert isinstance(result1, NFLState)
        assert isinstance(result2, NFLState)
        assert result1.model_dump() == result2.model_dump()


@pytest.mark.asyncio
async def test_error_handling(mock_config: Config):
    """Test error handling for API requests."""
    with patch("httpx.AsyncClient.request") as mock_request:
        # Simulate a network error
        mock_request.side_effect = httpx.RequestError("Network error")
        
        client = SleeperAPIClient(mock_config)
        
        with pytest.raises(HTTPException) as exc_info:
            await client.get_nfl_state()
        
        assert exc_info.value.status_code == 500
        assert "Failed to communicate with Sleeper API" in str(exc_info.value.detail)
