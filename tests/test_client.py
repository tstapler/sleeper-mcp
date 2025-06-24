"""Test suite for Sleeper API client."""

import pytest
from sleeper_mcp.sleeper_api import SleeperClient
from sleeper_mcp.sleeper_api.models.base import User, League, Roster

TEST_BOT_USERNAME = "sleeperbot"  # Known bot user
TEST_BOT_USER_ID = "160004000000400000"

@pytest.mark.asyncio
async def test_get_user():
    """Test getting user information."""
    async with SleeperClient() as client:
        data = await client.get_user(TEST_BOT_USERNAME)
        assert data is not None
        user = User(**data)
        assert user.username == TEST_BOT_USERNAME
        assert user.user_id == TEST_BOT_USER_ID
        assert user.is_bot is True

@pytest.mark.asyncio
async def test_get_user_leagues():
    """Test getting user leagues."""
    async with SleeperClient() as client:
        data = await client.get_user_leagues(TEST_BOT_USER_ID)
        assert isinstance(data, list)
        if data:  # Bot may not have leagues
            leagues = [League(**league_data) for league_data in data]
            assert all(isinstance(league, League) for league in leagues)

@pytest.mark.asyncio
async def test_get_league():
    """Test getting league information - skip if no leagues."""
    pytest.skip("Test requires a valid league ID from a user with leagues")
