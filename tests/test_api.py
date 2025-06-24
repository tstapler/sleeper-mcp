"""Test suite for FastAPI endpoints."""

from fastapi.testclient import TestClient
import pytest
from sleeper_mcp import app

client = TestClient(app)

TEST_BOT_USERNAME = "sleeperbot"  # Known bot user
TEST_BOT_USER_ID = "160004000000400000"

def test_get_user():
    """Test GET /users/{username} endpoint."""
    response = client.get(f"/users/{TEST_BOT_USERNAME}")
    assert response.status_code == 200
    data = response.json()
    assert data["username"] == TEST_BOT_USERNAME
    assert data["user_id"] == TEST_BOT_USER_ID
    assert data["is_bot"] is True

def test_get_user_leagues():
    """Test GET /users/{user_id}/leagues endpoint."""
    response = client.get(f"/users/{TEST_BOT_USER_ID}/leagues")
    assert response.status_code == 200
    data = response.json()
    assert isinstance(data, list)

def test_get_league():
    """Test GET /leagues/{league_id} endpoint."""
    pytest.skip("Test requires a valid league ID from a user with leagues")

def test_get_league_users():
    """Test GET /leagues/{league_id}/users endpoint."""
    pytest.skip("Test requires a valid league ID from a user with leagues")

def test_get_league_rosters():
    """Test GET /leagues/{league_id}/rosters endpoint."""
    pytest.skip("Test requires a valid league ID from a user with leagues")
