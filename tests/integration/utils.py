"""Test utilities and helpers for integration testing."""

import json
import os
from pathlib import Path
from typing import Any, Dict, Optional

import httpx
import pytest
from pytest_asyncio.plugin import Mode

# Constants for test data paths
TEST_DATA_DIR = Path(__file__).parent / "data"


def load_test_data(filename: str) -> Dict[str, Any]:
    """Load test data from JSON file.
    
    Args:
        filename: Name of the JSON file in the test data directory
        
    Returns:
        Dict[str, Any]: Loaded test data
        
    Raises:
        FileNotFoundError: If the test data file doesn't exist
    """
    file_path = TEST_DATA_DIR / filename
    if not file_path.exists():
        raise FileNotFoundError(f"Test data file not found: {filename}")
        
    with open(file_path, "r") as f:
        return json.load(f)


class MockResponse:
    """Mock HTTP response for testing."""

    def __init__(
        self,
        status_code: int = 200,
        json_data: Optional[Dict[str, Any]] = None,
        headers: Optional[Dict[str, str]] = None
    ):
        """Initialize mock response.
        
        Args:
            status_code: HTTP status code
            json_data: JSON response data
            headers: Response headers
        """
        self.status_code = status_code
        self._json_data = json_data or {}
        self.headers = headers or {}

    def json(self) -> Dict[str, Any]:
        """Get JSON response data.
        
        Returns:
            Dict[str, Any]: JSON response data
        """
        return self._json_data

    def raise_for_status(self):
        """Raise an exception if status code indicates an error."""
        if self.status_code >= 400:
            raise httpx.HTTPStatusError(
                f"HTTP error occurred: {self.status_code}",
                request=httpx.Request("GET", "http://test"),
                response=self
            )


async def async_return(value: Any) -> Any:
    """Helper to return a value in an async context.
    
    Args:
        value: Value to return
        
    Returns:
        Any: The input value
    """
    return value


def pytest_configure(config):
    """Configure pytest for async testing.
    
    Args:
        config: pytest configuration object
    """
    config.option.asyncio_mode = Mode.STRICT
