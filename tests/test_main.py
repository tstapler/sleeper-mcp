"""Test the FastAPI application endpoints."""

import pytest
from fastapi.testclient import TestClient
from httpx import AsyncClient
import pytest_asyncio

from src.main import app


@pytest.fixture
def test_client():
    """Create a test client for the FastAPI application."""
    return TestClient(app)


@pytest_asyncio.fixture
async def async_client():
    """Create an async test client for the FastAPI application."""
    async with AsyncClient(app=app, base_url="http://test") as client:
        yield client


@pytest.mark.asyncio
async def test_health_check(async_client: AsyncClient):
    """Test the health check endpoint."""
    response = await async_client.get("/health")
    assert response.status_code == 200
    
    data = response.json()
    assert "status" in data
    assert "message" in data
    assert isinstance(data["status"], str)
    assert isinstance(data["message"], str)
